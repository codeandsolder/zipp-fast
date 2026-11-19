//! Built-in native HTTP/1.1 SAPI server for PHP backend apps (`zipp php -S host:port [-t docroot] [router.php]`)
//! and long-lived Worker Mode (`zipp php --worker -S host:port app.php`).
//!
//! Pure `std::net` + `serde_json` (zero extra dependencies):
//! - Default mode: shared-nothing per-request isolation (fresh `ScriptState` per
//!   request backed by ZIPP's memoized runtime seed + in-process OPcache).
//! - Worker mode (`--worker`): boots the PHP entry script once per worker thread
//!   (registering a callback via `zipp_handle_requests($fn)`) and dispatches
//!   requests directly into the warm VM.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use zipp_vm::embed::JsValue;
use zipp_vm::frontend::{compile_php_program, PhpProgramConfig};

pub(crate) struct ServerConfig {
    pub addr: String,
    pub docroot: PathBuf,
    pub router: Option<PathBuf>,
    pub worker_mode: bool,
}

pub(crate) fn serve(cfg: ServerConfig) -> Result<(), String> {
    let listener = TcpListener::bind(&cfg.addr)
        .map_err(|e| format!("failed to bind {}: {e}", cfg.addr))?;
    let local_addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| cfg.addr.clone());

    let docroot = std::fs::canonicalize(&cfg.docroot).unwrap_or_else(|_| cfg.docroot.clone());
    let router = cfg
        .router
        .as_ref()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()));

    let mode_label = if cfg.worker_mode { "worker" } else { "sapi" };
    eprintln!(
        "ZIPP PHP 8.4 Server ({mode_label}) listening on http://{local_addr} (docroot: {})",
        docroot.display()
    );

    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 16))
        .unwrap_or(4);

    if cfg.worker_mode {
        let entry = router
            .ok_or("zipp php --worker -S requires an entry PHP script")?;
        return serve_worker_pool(listener, entry, docroot, worker_count);
    }

    let (tx, rx) = mpsc::channel::<TcpStream>();
    let rx = Arc::new(Mutex::new(rx));
    let docroot = Arc::new(docroot);
    let router = Arc::new(router);

    for i in 0..worker_count {
        let rx = Arc::clone(&rx);
        let docroot = Arc::clone(&docroot);
        let router = Arc::clone(&router);
        let addr_label = local_addr.clone();
        std::thread::Builder::new()
            .name(format!("zipp-php-{i}"))
            .stack_size(16 * 1024 * 1024)
            .spawn(move || loop {
                let stream = {
                    let guard = rx.lock().unwrap();
                    guard.recv()
                };
                match stream {
                    Ok(s) => handle_connection(s, &docroot, router.as_deref(), &addr_label),
                    Err(_) => break,
                }
            })
            .map_err(|e| format!("failed to spawn worker thread: {e}"))?;
    }

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let _ = tx.send(stream);
            }
            Err(e) => {
                eprintln!("zipp php server accept error: {e}");
            }
        }
    }
    Ok(())
}

fn serve_worker_pool(
    listener: TcpListener,
    entry: PathBuf,
    docroot: PathBuf,
    worker_count: usize,
) -> Result<(), String> {
    let source = std::fs::read_to_string(&entry)
        .map_err(|e| format!("{}: {e}", entry.display()))?;
    let local_addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();

    let (tx, rx) = mpsc::channel::<TcpStream>();
    let rx = Arc::new(Mutex::new(rx));
    let source = Arc::new(source);
    let entry = Arc::new(entry);
    let docroot = Arc::new(docroot);

    for i in 0..worker_count {
        let rx = Arc::clone(&rx);
        let source = Arc::clone(&source);
        let entry = Arc::clone(&entry);
        let docroot = Arc::clone(&docroot);
        let addr_label = local_addr.clone();
        std::thread::Builder::new()
            .name(format!("zipp-php-worker-{i}"))
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let file_str = normalize_path(&entry);
                let base_dir = entry
                    .parent()
                    .map(normalize_path)
                    .unwrap_or_else(|| normalize_path(&docroot));
                let mut compiled = match compile_php_program(
                    &source,
                    PhpProgramConfig {
                        file: file_str,
                        base_dir,
                        allow_native_fs: true,
                        stream_stdout: false,
                        ..Default::default()
                    },
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("zipp php worker compile error: {e}");
                        return;
                    }
                };
                if let Err(e) = compiled.state_mut().run_init() {
                    eprintln!("zipp php worker boot error: {e}");
                    return;
                }
                let _ = compiled.state_mut().take_php_output();

                loop {
                    let stream = {
                        let guard = rx.lock().unwrap();
                        guard.recv()
                    };
                    let Ok(mut stream) = stream else { break };
                    let Ok(Some(req)) = parse_http_request(&mut stream) else {
                        continue;
                    };
                    let req_json = build_sapi_request_json(&req, &entry, &docroot, &addr_label, "zipp-worker");
                    let resp_meta = compiled
                        .state_mut()
                        .call_global("__zipp_php_worker_request", &[JsValue::String(req_json)]);
                    let body = compiled.state_mut().take_php_output();
                    match resp_meta {
                        Ok(JsValue::String(meta_json)) => {
                            write_sapi_http_response(&mut stream, &meta_json, &body);
                        }
                        Ok(_) => {
                            write_plain_response(&mut stream, 500, "text/plain", b"Invalid worker response");
                        }
                        Err(e) => {
                            write_plain_response(&mut stream, 500, "text/plain", e.as_bytes());
                        }
                    }
                }
            })
            .map_err(|e| format!("failed to spawn worker thread: {e}"))?;
    }

    for incoming in listener.incoming() {
        if let Ok(stream) = incoming {
            let _ = tx.send(stream);
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    docroot: &Path,
    router: Option<&Path>,
    local_addr: &str,
) {
    let Ok(Some(req)) = parse_http_request(&mut stream) else {
        return;
    };

    // Resolve target script or static file safely under docroot.
    let rel_path = req.path.trim_start_matches('/');
    let safe_rel = sanitize_rel_path(rel_path);
    let candidate = docroot.join(&safe_rel);

    let script_path = if let Some(r) = router {
        // If requesting an existing non-PHP static file under docroot, serve it directly
        if !safe_rel.as_os_str().is_empty()
            && candidate.is_file()
            && !is_php_file(&candidate)
        {
            serve_static_file(&mut stream, &candidate);
            return;
        }
        r.to_path_buf()
    } else if candidate.is_file() {
        if !is_php_file(&candidate) {
            serve_static_file(&mut stream, &candidate);
            return;
        }
        candidate
    } else if candidate.is_dir() && candidate.join("index.php").is_file() {
        candidate.join("index.php")
    } else if candidate.is_dir() && candidate.join("index.html").is_file() {
        serve_static_file(&mut stream, &candidate.join("index.html"));
        return;
    } else {
        write_plain_response(&mut stream, 404, "text/plain; charset=UTF-8", b"404 Not Found");
        return;
    };

    let source = match std::fs::read_to_string(&script_path) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("500 Internal Server Error: {e}");
            write_plain_response(&mut stream, 500, "text/plain; charset=UTF-8", msg.as_bytes());
            return;
        }
    };

    let req_json = build_sapi_request_json(&req, &script_path, docroot, local_addr, "cli-server");
    let file_str = normalize_path(&script_path);
    let base_dir = script_path
        .parent()
        .map(normalize_path)
        .unwrap_or_else(|| normalize_path(docroot));

    let mut compiled = match compile_php_program(
        &source,
        PhpProgramConfig {
            file: file_str,
            base_dir,
            sapi_request_json: Some(req_json),
            allow_native_fs: true,
            stream_stdout: false,
            ..Default::default()
        },
    ) {
        Ok(c) => c,
        Err(e) => {
            write_plain_response(&mut stream, 500, "text/plain; charset=UTF-8", e.as_bytes());
            return;
        }
    };

    let run_res = compiled.state_mut().run_init();
    let body = compiled.state_mut().take_php_output();
    if let Err(err) = run_res {
        let mut out = body;
        if !out.is_empty() {
            out.push(b'\n');
        }
        out.extend_from_slice(err.as_bytes());
        write_plain_response(&mut stream, 500, "text/plain; charset=UTF-8", &out);
        return;
    }

    match compiled.state_mut().call_global("__zipp_php_take_response", &[]) {
        Ok(JsValue::String(meta_json)) => {
            write_sapi_http_response(&mut stream, &meta_json, &body);
        }
        _ => {
            write_plain_response(&mut stream, 200, "text/html; charset=UTF-8", &body);
        }
    }
}

struct HttpRequest {
    method: String,
    uri: String,
    path: String,
    query: String,
    protocol: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    remote_addr: String,
}

fn parse_http_request(stream: &mut TcpStream) -> std::io::Result<Option<HttpRequest>> {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));
    let remote_addr = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_owned());

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line)?;
    if n == 0 {
        return Ok(None);
    }
    let parts: Vec<&str> = request_line.trim_end().split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(None);
    }
    let method = parts[0].to_ascii_uppercase();
    let uri = parts[1].to_owned();
    let protocol = parts.get(2).copied().unwrap_or("HTTP/1.1").to_owned();
    let (path, query) = match uri.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (uri.clone(), String::new()),
    };

    let mut headers = HashMap::new();
    let mut content_len: usize = 0;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let lower = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            if lower == "content-length" {
                content_len = val.parse::<usize>().unwrap_or(0).min(16 * 1024 * 1024);
            }
            headers.insert(lower, val);
        }
    }

    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        reader.read_exact(&mut body)?;
    }

    Ok(Some(HttpRequest {
        method,
        uri,
        path,
        query,
        protocol,
        headers,
        body,
        remote_addr,
    }))
}

fn build_sapi_request_json(
    req: &HttpRequest,
    script: &Path,
    docroot: &Path,
    local_addr: &str,
    sapi: &str,
) -> String {
    let mut server = serde_json::Map::new();
    let (host, port) = req
        .headers
        .get("host")
        .map(|h| match h.rsplit_once(':') {
            Some((a, b)) => (a.to_owned(), b.to_owned()),
            None => (h.clone(), "80".to_owned()),
        })
        .unwrap_or_else(|| match local_addr.rsplit_once(':') {
            Some((a, b)) => (a.to_owned(), b.to_owned()),
            None => ("localhost".to_owned(), "8000".to_owned()),
        });

    let script_str = normalize_path(script);
    let docroot_str = normalize_path(docroot);
    server.insert("REQUEST_METHOD".into(), req.method.clone().into());
    server.insert("REQUEST_URI".into(), req.uri.clone().into());
    server.insert("PATH_INFO".into(), req.path.clone().into());
    server.insert("QUERY_STRING".into(), req.query.clone().into());
    server.insert("SERVER_PROTOCOL".into(), req.protocol.clone().into());
    server.insert("SERVER_NAME".into(), host.into());
    server.insert("SERVER_PORT".into(), port.into());
    server.insert("REMOTE_ADDR".into(), req.remote_addr.clone().into());
    server.insert("DOCUMENT_ROOT".into(), docroot_str.into());
    server.insert("SCRIPT_FILENAME".into(), script_str.clone().into());
    server.insert("SCRIPT_NAME".into(), req.path.clone().into());
    server.insert("PHP_SELF".into(), req.path.clone().into());
    server.insert("SERVER_SOFTWARE".into(), "ZIPP PHP Server".into());

    let content_type = req.headers.get("content-type").cloned().unwrap_or_default();
    if !content_type.is_empty() {
        server.insert("CONTENT_TYPE".into(), content_type.clone().into());
    }
    if !req.body.is_empty() {
        server.insert("CONTENT_LENGTH".into(), req.body.len().to_string().into());
    }
    for (k, v) in &req.headers {
        let http_key = format!("HTTP_{}", k.replace('-', "_").to_ascii_uppercase());
        server.insert(http_key, v.clone().into());
    }

    let mut cookies = serde_json::Map::new();
    if let Some(cookie_hdr) = req.headers.get("cookie") {
        for pair in cookie_hdr.split(';') {
            if let Some((k, v)) = pair.trim().split_once('=') {
                cookies.insert(k.trim().to_owned(), v.trim().to_owned().into());
            }
        }
    }

    // Encode body as Latin-1 JS string so binary bytes round-trip cleanly into PHP byte-strings!
    let body_bstr: String = req.body.iter().map(|&b| char::from(b)).collect();

    let payload = serde_json::json!({
        "sapi": sapi,
        "server": server,
        "query": req.query,
        "cookies": cookies,
        "contentType": content_type,
        "body": body_bstr,
    });
    payload.to_string()
}

fn write_sapi_http_response(stream: &mut TcpStream, meta_json: &str, body: &[u8]) {
    let parsed: serde_json::Value = serde_json::from_str(meta_json).unwrap_or_default();
    let status = parsed.get("status").and_then(|v| v.as_u64()).unwrap_or(200) as u16;
    let reason = http_reason(status);

    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    let mut has_cl = false;
    if let Some(hdrs) = parsed.get("headers").and_then(|v| v.as_array()) {
        for pair in hdrs {
            if let (Some(k), Some(v)) = (
                pair.get(0).and_then(|x| x.as_str()),
                pair.get(1).and_then(|x| x.as_str()),
            ) {
                if k.eq_ignore_ascii_case("content-length") {
                    has_cl = true;
                }
                // Strip CR/LF to prevent response splitting
                let ck = k.replace(['\r', '\n'], "");
                let cv = v.replace(['\r', '\n'], "");
                head.push_str(&format!("{ck}: {cv}\r\n"));
            }
        }
    }
    if !has_cl {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn write_plain_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = http_reason(status);
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn serve_static_file(stream: &mut TcpStream, path: &Path) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mime = match path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
                .as_deref()
            {
                Some("html" | "htm") => "text/html; charset=UTF-8",
                Some("css") => "text/css; charset=UTF-8",
                Some("js" | "mjs") => "application/javascript; charset=UTF-8",
                Some("json") => "application/json; charset=UTF-8",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("ico") => "image/x-icon",
                Some("wasm") => "application/wasm",
                Some("txt") => "text/plain; charset=UTF-8",
                _ => "application/octet-stream",
            };
            write_plain_response(stream, 200, mime, &bytes);
        }
        Err(_) => write_plain_response(stream, 404, "text/plain; charset=UTF-8", b"404 Not Found"),
    }
}

fn sanitize_rel_path(rel: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') || seg.contains(':') {
            continue;
        }
        out.push(seg);
    }
    out
}

fn is_php_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "php" | "phtml"))
}

fn normalize_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_owned()
}

fn http_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}
