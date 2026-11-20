//! Built-in multi-threaded HTTP/1.1 Worker Server for Rust backend apps
//! (`zipp rs --serve HOST:PORT app.rs` or `zipp rs -S HOST:PORT app.rs`).
//!
//! Pure `std::net` + `serde_json` (zero extra dependencies):
//! Boots `app.rs` once per worker thread in `worker_boot` mode (calling `fn router() -> Router`
//! or `fn main()` to register the `Router`), then dispatches incoming HTTP/1.1 requests
//! directly into each warm ZIPP VM with full JIT acceleration.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use zipp_vm::embed::JsValue;
use zipp_vm::frontend::{compile_rust_program, CompiledSource, RustProgramConfig};

pub(crate) struct ServerConfig {
    pub addr: String,
    pub entry: PathBuf,
    pub workers: Option<usize>,
    pub watch: bool,
}

fn boot_worker_vm(entry: &Path, source: &str) -> Result<CompiledSource, String> {
    let file_str = normalize_path(entry);
    let base_dir = entry
        .parent()
        .map(normalize_path)
        .unwrap_or_else(|| ".".to_owned());
    let mut compiled = compile_rust_program(
        source,
        RustProgramConfig {
            file: file_str,
            base_dir,
            allow_native_fs: true,
            allow_net: true,
            stream_stdout: false,
            mode: "worker_boot".to_owned(),
            ..Default::default()
        },
    )?;
    compiled.state_mut().run_init()?;
    let _ = compiled.state_mut().take_rust_output();
    Ok(compiled)
}

pub(crate) fn serve(cfg: ServerConfig) -> Result<(), String> {
    let resolved_entry = super::frontend_cli::resolve_rust_entry(&cfg.entry, false)?;
    let entry = std::fs::canonicalize(&resolved_entry).unwrap_or(resolved_entry);
    let source = std::fs::read_to_string(&entry)
        .map_err(|e| format!("{}: {e}", entry.display()))?;

    let listener = TcpListener::bind(&cfg.addr)
        .map_err(|e| format!("failed to bind {}: {e}", cfg.addr))?;
    let local_addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| cfg.addr.clone());

    let worker_count = cfg.workers.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 16))
            .unwrap_or(4)
    });

    eprintln!(
        "ZIPP Rust Worker Server ({worker_count} workers{}) listening on http://{local_addr} ({})",
        if cfg.watch { ", live-reload" } else { "" },
        entry.display()
    );

    let (tx, rx) = mpsc::channel::<TcpStream>();
    let rx = Arc::new(Mutex::new(rx));
    let shared_source = Arc::new(RwLock::new(source));
    let generation = Arc::new(AtomicU64::new(0));
    let entry = Arc::new(entry);

    if cfg.watch {
        let watch_root = super::frontend_cli::rust_watch_root(&entry);
        let watch_entry = Arc::clone(&entry);
        let watch_source = Arc::clone(&shared_source);
        let watch_gen = Arc::clone(&generation);
        let mut snap = super::frontend_cli::snapshot_rust_sources(&watch_root);
        eprintln!(
            "[zipp rs --serve --watch] watching {} ({} files) — edit any .rs file to hot-reload workers",
            watch_root.display(),
            snap.len()
        );
        std::thread::Builder::new()
            .name("zipp-rs-watcher".to_owned())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let next = super::frontend_cli::snapshot_rust_sources(&watch_root);
                if let Some(changed) = super::frontend_cli::diff_rust_snapshot(&snap, &next) {
                    snap = next;
                    if let Ok(fresh) = std::fs::read_to_string(&*watch_entry) {
                        if let Ok(mut guard) = watch_source.write() {
                            *guard = fresh;
                        }
                        watch_gen.fetch_add(1, Ordering::Release);
                        let label = changed
                            .strip_prefix(&watch_root)
                            .unwrap_or(&changed)
                            .display();
                        eprintln!("[zipp rs --serve --watch] hot-reloading ({label})");
                    }
                }
            })
            .map_err(|e| format!("failed to spawn Rust watch thread: {e}"))?;
    }

    for i in 0..worker_count {
        let rx = Arc::clone(&rx);
        let shared_source = Arc::clone(&shared_source);
        let generation = Arc::clone(&generation);
        let entry = Arc::clone(&entry);
        std::thread::Builder::new()
            .name(format!("zipp-rs-worker-{i}"))
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let mut local_gen = generation.load(Ordering::Acquire);
                let initial_src = shared_source
                    .read()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                let mut compiled: Result<CompiledSource, String> =
                    boot_worker_vm(&entry, &initial_src);
                if let Err(ref e) = compiled {
                    eprintln!("zipp rs worker boot error: {e}");
                    if !cfg.watch {
                        return;
                    }
                }

                loop {
                    let stream = {
                        let guard = rx.lock().unwrap();
                        guard.recv()
                    };
                    match stream {
                        Ok(mut s) => {
                            let cur_gen = generation.load(Ordering::Acquire);
                            if cur_gen != local_gen {
                                local_gen = cur_gen;
                                let fresh_src = shared_source
                                    .read()
                                    .map(|g| g.clone())
                                    .unwrap_or_default();
                                compiled = boot_worker_vm(&entry, &fresh_src);
                                if let Err(ref e) = compiled {
                                    eprintln!("zipp rs worker hot-reload error: {e}");
                                }
                            }
                            match compiled.as_mut() {
                                Ok(prog) => handle_worker_connection(s, prog.state_mut()),
                                Err(e) => {
                                    let _ = read_http_request(&mut s);
                                    let err_json = serde_json::json!({
                                        "status": 500,
                                        "headers": [["Content-Type", "text/plain; charset=utf-8"]],
                                        "body": format!("Rust hot-reload error: {e}")
                                    })
                                    .to_string();
                                    write_http_response(&mut s, &err_json);
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("failed to spawn Rust worker thread: {e}"))?;
    }

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let _ = tx.send(stream);
            }
            Err(e) => {
                eprintln!("zipp rs server accept error: {e}");
            }
        }
    }
    Ok(())
}

fn handle_worker_connection(mut stream: TcpStream, state: &mut zipp_vm::embed::ScriptState) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let Some(req) = read_http_request(&mut stream) else {
        return;
    };
    let req_json = serde_json::json!({
        "method": req.method,
        "uri": req.uri,
        "headers": req.headers,
        "body": req.body,
    })
    .to_string();

    let resp_json = match state.call_global("__zipp_rs_worker_request", &[JsValue::String(req_json)]) {
        Ok(JsValue::String(s)) => s,
        Ok(_) => serde_json::json!({
            "status": 500,
            "headers": [["Content-Type", "text/plain"]],
            "body": "invalid response from Rust worker"
        })
        .to_string(),
        Err(e) => serde_json::json!({
            "status": 500,
            "headers": [["Content-Type", "text/plain"]],
            "body": format!("Rust worker error: {e}")
        })
        .to_string(),
    };
    write_http_response(&mut stream, &resp_json);
}

struct ParsedHttpReq {
    method: String,
    uri: String,
    headers: HashMap<String, String>,
    body: String,
}

fn read_http_request(stream: &mut TcpStream) -> Option<ParsedHttpReq> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let parts: Vec<&str> = line.trim_end().split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let method = parts[0].to_owned();
    let uri = parts[1].to_owned();
    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    loop {
        if headers.len() >= 256 {
            return None;
        }
        let mut hline = String::new();
        if reader.read_line(&mut hline).ok()? == 0 {
            break;
        }
        let trimmed = hline.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            let vv = v.trim().to_owned();
            if kl == "content-length" {
                content_length = vv.parse().unwrap_or(0).min(16 * 1024 * 1024);
            }
            headers.insert(kl, vv);
        }
    }
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body_bytes);
    }
    Some(ParsedHttpReq {
        method,
        uri,
        headers,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
    })
}

fn write_http_response(stream: &mut TcpStream, resp_json: &str) {
    let parsed: serde_json::Value = serde_json::from_str(resp_json).unwrap_or_else(|_| {
        serde_json::json!({ "status": 500, "headers": [], "body": "malformed worker response" })
    });
    let status = parsed.get("status").and_then(|v| v.as_u64()).unwrap_or(200) as u16;
    let body = parsed
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .as_bytes();
    let reason = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
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
    };
    let mut wire = format!("HTTP/1.1 {status} {reason}\r\n");
    let mut has_ct = false;
    let mut has_cl = false;
    if let Some(arr) = parsed.get("headers").and_then(|v| v.as_array()) {
        for pair in arr {
            if let Some(p) = pair.as_array() {
                if let (Some(k), Some(v)) = (p.first().and_then(|x| x.as_str()), p.get(1).and_then(|x| x.as_str())) {
                    if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
                        continue;
                    }
                    if k.eq_ignore_ascii_case("content-type") {
                        has_ct = true;
                    }
                    if k.eq_ignore_ascii_case("content-length") {
                        has_cl = true;
                    }
                    wire.push_str(&format!("{k}: {v}\r\n"));
                }
            }
        }
    }
    if !has_ct {
        wire.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    }
    if !has_cl {
        wire.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    wire.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(wire.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn normalize_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_owned()
}
