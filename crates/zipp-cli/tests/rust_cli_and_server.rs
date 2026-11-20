//! End-to-end CLI and live HTTP/1.1 worker server tests for `zipp rs`.
#![cfg(feature = "rust")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

fn http_send(port: u16, req: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to zipp rs server");
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp
}

#[test]
fn cli_runs_rust_script_autodetects_rs_and_runs_tests() {
    let exe = env!("CARGO_BIN_EXE_zipp");
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let hello = repo_root.join("examples/rust/hello.rs");

    // 1. `zipp rs examples/rust/hello.rs foo bar`
    let out1 = Command::new(exe)
        .args(["rs", hello.to_str().unwrap(), "foo", "bar"])
        .output()
        .expect("run zipp rs");
    assert!(
        out1.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let s1 = String::from_utf8_lossy(&out1.stdout);
    assert!(s1.contains("Completed 2/3 tasks"), "{s1}");
    assert!(s1.contains("'runs' count = 2"), "{s1}");
    assert!(s1.contains("fib(25) = 75025"), "{s1}");
    assert!(s1.contains("\"foo\", \"bar\""), "{s1}");

    // 2. Direct auto-detection: `zipp examples/rust/hello.rs`
    let out2 = Command::new(exe)
        .arg(hello.to_str().unwrap())
        .output()
        .expect("run zipp hello.rs");
    assert!(out2.status.success());
    assert!(String::from_utf8_lossy(&out2.stdout).contains("fib(25) = 75025"));

    // 3. Built-in Rust test runner: `zipp rs --test examples/rust/hello.rs`
    let out3 = Command::new(exe)
        .args(["rs", "--test", hello.to_str().unwrap()])
        .output()
        .expect("run zipp rs --test");
    assert!(
        out3.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out3.stdout),
        String::from_utf8_lossy(&out3.stderr)
    );
    let s3 = String::from_utf8_lossy(&out3.stdout);
    assert!(s3.contains("test test_fib ... ok"), "{s3}");
    assert!(s3.contains("test result: ok. 1 passed; 0 failed"), "{s3}");

    // 4. Bytecode inspection: `zipp rs --bc examples/rust/hello.rs`
    let out4 = Command::new(exe)
        .args(["rs", "--bc", hello.to_str().unwrap()])
        .output()
        .expect("run zipp rs --bc");
    assert!(out4.status.success());
    assert!(String::from_utf8_lossy(&out4.stdout).contains("__rs_fn_fib"));
}

#[test]
fn cli_rust_worker_http_server_serves_multifile_api() {
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_zipp");
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let server = repo_root.join("examples/rust/backend_api/server.rs");

    let mut child = Command::new(exe)
        .args([
            "rs",
            "--serve",
            &format!("127.0.0.1:{port}"),
            "--workers",
            "1",
            server.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zipp rs --serve");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // 1. GET / -> HTML
    let r1 = http_send(port, "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(r1.starts_with("HTTP/1.1 200 OK"), "r1:\n{r1}");
    assert!(r1.contains("<h1>ZIPP Rust Backend API</h1>"), "r1:\n{r1}");
    assert!(r1.contains("X-Runtime: ZIPP-Rust"), "r1:\n{r1}");

    // 2. GET /health -> JSON state
    let r2 = http_send(port, "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(r2.starts_with("HTTP/1.1 200 OK"), "r2:\n{r2}");
    assert!(r2.contains("\"status\":\"ok\""), "r2:\n{r2}");
    assert!(r2.contains("\"item_count\":2"), "r2:\n{r2}");

    // 3. GET /api/items/item-1 -> path param lookup
    let r3 = http_send(
        port,
        "GET /api/items/item-1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(r3.starts_with("HTTP/1.1 200 OK"), "r3:\n{r3}");
    assert!(r3.contains("\"name\":\"Widget\""), "r3:\n{r3}");

    // 4. POST /api/items -> 201 Created + state mutation persists in worker VM!
    let payload = r#"{"name":"Bolt","quantity":99}"#;
    let r4 = http_send(
        port,
        &format!(
            "POST /api/items HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        ),
    );
    assert!(r4.starts_with("HTTP/1.1 201 Created"), "r4:\n{r4}");
    assert!(r4.contains("\"name\":\"Bolt\""), "r4:\n{r4}");
    assert!(r4.contains("\"quantity\":99"), "r4:\n{r4}");

    // 5. GET /api/items -> now has 3 items
    let r5 = http_send(
        port,
        "GET /api/items HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(r5.starts_with("HTTP/1.1 200 OK"), "r5:\n{r5}");
    assert!(r5.contains("\"Bolt\""), "r5:\n{r5}");

    // 6. GET /api/items/nonexistent -> 404 Not Found
    let r6 = http_send(
        port,
        "GET /api/items/nonexistent HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(r6.starts_with("HTTP/1.1 404 Not Found"), "r6:\n{r6}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cli_in_script_router_serve_and_stdin_mode() {
    let exe = env!("CARGO_BIN_EXE_zipp");

    // 1. Stdin execution via `zipp --lang=rs -`
    let mut stdin_child = Command::new(exe)
        .args(["--lang=rs", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zipp --lang=rs -");
    stdin_child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"fn main() { println!(\"from-stdin:{}\", 6 * 7); }\n")
        .unwrap();
    let stdin_out = stdin_child.wait_with_output().unwrap();
    assert!(
        stdin_out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&stdin_out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&stdin_out.stdout), "from-stdin:42\n");

    // 2. Direct in-script `Router::serve()` via `zipp rs examples/rust/backend_api/server.rs 127.0.0.1:<port>`
    let port = find_free_port();
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let server = repo_root.join("examples/rust/backend_api/server.rs");

    let mut child = Command::new(exe)
        .args(["rs", server.to_str().unwrap(), &format!("127.0.0.1:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn direct zipp rs server.rs");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let r = http_send(
        port,
        "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(r.starts_with("HTTP/1.1 200 OK"), "direct serve resp:\n{r}");
    assert!(r.contains("\"runtime\":\"zipp-rust\""), "direct serve resp:\n{r}");

    let _ = child.kill();
    let _ = child.wait();
}

fn http_req(port: u16, method: &str, path: &str, token: Option<&str>, body: Option<&str>) -> String {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    if let Some(b) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
        req.push_str("Connection: close\r\n\r\n");
        req.push_str(b);
    } else {
        req.push_str("Connection: close\r\n\r\n");
    }
    http_send(port, &req)
}

fn extract_json_str(body: &str, key: &str) -> String {
    let pat = format!("\"{key}\":\"");
    let start = body
        .find(&pat)
        .unwrap_or_else(|| panic!("missing key `{key}` in:\n{body}"))
        + pat.len();
    let end = body[start..]
        .find('"')
        .unwrap_or_else(|| panic!("unterminated string for `{key}` in:\n{body}"));
    body[start..start + end].to_owned()
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not start on port {port} in time");
}

#[test]
fn cli_full_cargo_issue_tracker_app_end_to_end_and_persistence() {
    let exe = env!("CARGO_BIN_EXE_zipp");
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let app_dir = repo_root.join("examples/rust/issue_tracker");

    // 1. Run Cargo project unit & async tests directly via `zipp rs --test <dir>`
    let test_out = Command::new(exe)
        .args(["rs", "--test", app_dir.to_str().unwrap()])
        .output()
        .expect("run zipp rs --test on issue_tracker");
    assert!(
        test_out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&test_out.stdout),
        String::from_utf8_lossy(&test_out.stderr)
    );
    let ts = String::from_utf8_lossy(&test_out.stdout);
    assert!(ts.contains("test result: ok. 3 passed; 0 failed"), "{ts}");

    // 2. Launch the full-stack Rust app on a temp persistence file
    let tmp_dir = std::env::temp_dir().join(format!("zipp_issue_tracker_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let data_file = tmp_dir.join("issues.json");

    let port1 = find_free_port();
    let mut child1 = Command::new(exe)
        .args([
            "rs",
            "--serve",
            &format!("127.0.0.1:{port1}"),
            "--workers",
            "1",
            app_dir.to_str().unwrap(),
        ])
        .env("ISSUE_DATA_FILE", data_file.to_str().unwrap())
        .env("JWT_SECRET", "e2e-test-jwt-secret")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn issue_tracker server #1");
    wait_for_port(port1);

    // Static UI + CSS + Middleware headers
    let r_idx = http_req(port1, "GET", "/", None, None);
    assert!(r_idx.starts_with("HTTP/1.1 200 OK"), "{r_idx}");
    assert!(r_idx.contains("text/html"), "{r_idx}");
    assert!(r_idx.contains("ZIPP Rust Issue Tracker"), "{r_idx}");
    assert!(r_idx.contains("X-Runtime: ZIPP-Rust"), "{r_idx}");
    assert!(r_idx.contains("X-Request-Id:"), "{r_idx}");

    let r_css = http_req(port1, "GET", "/static/style.css", None, None);
    assert!(r_css.starts_with("HTTP/1.1 200 OK"), "{r_css}");
    assert!(r_css.contains("text/css"), "{r_css}");
    assert!(r_css.contains("--accent"), "{r_css}");

    // Health check validates Cargo.toml env!("CARGO_PKG_NAME") & env!("CARGO_PKG_VERSION")
    let r_health = http_req(port1, "GET", "/api/health", None, None);
    assert!(r_health.starts_with("HTTP/1.1 200 OK"), "{r_health}");
    assert!(r_health.contains("\"service\":\"issue-tracker\""), "{r_health}");
    assert!(r_health.contains("\"version\":\"0.1.0\""), "{r_health}");

    // Unauthenticated POST /api/issues -> 401 Unauthorized
    let r_unauth = http_req(
        port1,
        "POST",
        "/api/issues",
        None,
        Some(r#"{"title":"Blocked"}"#),
    );
    assert!(r_unauth.starts_with("HTTP/1.1 401 Unauthorized"), "{r_unauth}");

    // Register user -> 201 Created, no passwordHash leaked!
    let r_reg = http_req(
        port1,
        "POST",
        "/api/auth/register",
        None,
        Some(r#"{"username":"alice","password":"s3cr3tPass"}"#),
    );
    assert!(r_reg.starts_with("HTTP/1.1 201 Created"), "{r_reg}");
    assert!(!r_reg.contains("passwordHash"), "leaked passwordHash!\n{r_reg}");
    assert!(!r_reg.contains("$zsha256$"), "leaked hash!\n{r_reg}");
    assert!(r_reg.contains("\"createdAt\":"), "{r_reg}");
    assert!(r_reg.contains("\"role\":\"admin\""), "{r_reg}");

    // Duplicate registration -> 409 Conflict
    let r_dup = http_req(
        port1,
        "POST",
        "/api/auth/register",
        None,
        Some(r#"{"username":"ALICE","password":"anotherPass"}"#),
    );
    assert!(r_dup.starts_with("HTTP/1.1 409 Conflict"), "{r_dup}");

    // Login -> 200 OK + HS256 JWT
    let r_login = http_req(
        port1,
        "POST",
        "/api/auth/login",
        None,
        Some(r#"{"username":"alice","password":"s3cr3tPass"}"#),
    );
    assert!(r_login.starts_with("HTTP/1.1 200 OK"), "{r_login}");
    let token = extract_json_str(&r_login, "token");
    assert_eq!(token.split('.').count(), 3, "expected JWT with 3 parts: {token}");

    // GET /api/auth/me with Bearer token
    let r_me = http_req(port1, "GET", "/api/auth/me", Some(&token), None);
    assert!(r_me.starts_with("HTTP/1.1 200 OK"), "{r_me}");
    assert!(r_me.contains("\"username\":\"alice\""), "{r_me}");

    // Create two issues
    let r_i1 = http_req(
        port1,
        "POST",
        "/api/issues",
        Some(&token),
        Some(r#"{"title":"Verify JIT loop speed","description":"Check fib(25)","priority":"high","labels":["vm","rust"]}"#),
    );
    assert!(r_i1.starts_with("HTTP/1.1 201 Created"), "{r_i1}");
    assert!(r_i1.contains("\"status\":\"open\""), "{r_i1}");
    assert!(r_i1.contains("\"priority\":\"high\""), "{r_i1}");

    let r_i2 = http_req(
        port1,
        "POST",
        "/api/issues",
        Some(&token),
        Some(r#"{"title":"Polish docs","priority":"low"}"#),
    );
    assert!(r_i2.starts_with("HTTP/1.1 201 Created"), "{r_i2}");

    // Search & filter
    let r_q = http_req(port1, "GET", "/api/issues?status=open&q=jit", None, None);
    assert!(r_q.starts_with("HTTP/1.1 200 OK"), "{r_q}");
    assert!(r_q.contains("Verify JIT loop speed"), "{r_q}");
    assert!(!r_q.contains("Polish docs"), "{r_q}");

    // State transitions via PATCH (`open -> in_progress -> closed`, then reject `closed -> in_progress`)
    let r_p1 = http_req(
        port1,
        "PATCH",
        "/api/issues/1",
        Some(&token),
        Some(r#"{"status":"in_progress"}"#),
    );
    assert!(r_p1.starts_with("HTTP/1.1 200 OK"), "{r_p1}");
    assert!(r_p1.contains("\"status\":\"in_progress\""), "{r_p1}");

    let r_p2 = http_req(
        port1,
        "PATCH",
        "/api/issues/1",
        Some(&token),
        Some(r#"{"status":"closed"}"#),
    );
    assert!(r_p2.starts_with("HTTP/1.1 200 OK"), "{r_p2}");
    assert!(r_p2.contains("\"status\":\"closed\""), "{r_p2}");

    let r_p_bad = http_req(
        port1,
        "PATCH",
        "/api/issues/1",
        Some(&token),
        Some(r#"{"status":"in_progress"}"#),
    );
    assert!(r_p_bad.starts_with("HTTP/1.1 400 Bad Request"), "{r_p_bad}");

    // Add comment & check stats & delete #2
    let r_c = http_req(
        port1,
        "POST",
        "/api/issues/1/comments",
        Some(&token),
        Some(r#"{"body":"Ship it!"}"#),
    );
    assert!(r_c.starts_with("HTTP/1.1 201 Created"), "{r_c}");
    assert!(r_c.contains("\"author\":\"alice\""), "{r_c}");

    let r_stats = http_req(port1, "GET", "/api/issues/stats", None, None);
    assert!(r_stats.starts_with("HTTP/1.1 200 OK"), "{r_stats}");
    assert!(r_stats.contains("\"total\":2"), "{r_stats}");
    assert!(r_stats.contains("\"open\":1"), "{r_stats}");
    assert!(r_stats.contains("\"closed\":1"), "{r_stats}");

    let r_del = http_req(port1, "DELETE", "/api/issues/2", Some(&token), None);
    assert!(r_del.starts_with("HTTP/1.1 204 No Content"), "{r_del}");

    let _ = child1.kill();
    let _ = child1.wait();

    // 3. Restart server pointing at the SAME data file and verify full persistence!
    let port2 = find_free_port();
    let mut child2 = Command::new(exe)
        .args([
            "rs",
            "--serve",
            &format!("127.0.0.1:{port2}"),
            "--workers",
            "1",
            app_dir.to_str().unwrap(),
        ])
        .env("ISSUE_DATA_FILE", data_file.to_str().unwrap())
        .env("JWT_SECRET", "e2e-test-jwt-secret")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn issue_tracker server #2");
    wait_for_port(port2);

    let r_persist_issue = http_req(port2, "GET", "/api/issues/1", None, None);
    assert!(r_persist_issue.starts_with("HTTP/1.1 200 OK"), "{r_persist_issue}");
    assert!(r_persist_issue.contains("\"status\":\"closed\""), "{r_persist_issue}");
    assert!(r_persist_issue.contains("\"body\":\"Ship it!\""), "{r_persist_issue}");

    let r_persist_del = http_req(port2, "GET", "/api/issues/2", None, None);
    assert!(r_persist_del.starts_with("HTTP/1.1 404 Not Found"), "{r_persist_del}");

    let r_persist_login = http_req(
        port2,
        "POST",
        "/api/auth/login",
        None,
        Some(r#"{"username":"alice","password":"s3cr3tPass"}"#),
    );
    assert!(r_persist_login.starts_with("HTTP/1.1 200 OK"), "{r_persist_login}");

    let _ = child2.kill();
    let _ = child2.wait();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn cli_rust_serve_watch_and_direct_watch_hot_reload_on_edit() {
    let exe = env!("CARGO_BIN_EXE_zipp");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp_dir = std::env::temp_dir().join(format!("zipp_rs_watch_{unique}"));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // 1. Worker server hot-reload: `zipp rs --serve HOST:PORT --watch --workers 1 app.rs`
    let app_rs = tmp_dir.join("app.rs");
    let v1 = r#"
use zipp_http::{Router, Response};
pub fn router() -> Router {
    Router::new().get("/version", |_req| Response::text("v1"))
}
"#;
    let v2 = r#"
use zipp_http::{Router, Response};
pub fn router() -> Router {
    Router::new().get("/version", |_req| Response::text("v2-reloaded"))
}
"#;
    std::fs::write(&app_rs, v1).unwrap();

    let port = find_free_port();
    let mut child = Command::new(exe)
        .args([
            "rs",
            "--serve",
            &format!("127.0.0.1:{port}"),
            "--watch",
            "--workers",
            "1",
            app_rs.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zipp rs --serve --watch");
    wait_for_port(port);

    let r1 = http_req(port, "GET", "/version", None, None);
    assert!(r1.starts_with("HTTP/1.1 200 OK"), "r1:\n{r1}");
    assert!(r1.ends_with("v1"), "r1:\n{r1}");

    // Edit the `.rs` file in-place without restarting `zipp` — ensure mtime advances.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(&app_rs, v2).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_v2 = false;
    while Instant::now() < deadline {
        let r = http_req(port, "GET", "/version", None, None);
        if r.ends_with("v2-reloaded") {
            saw_v2 = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(saw_v2, "zipp rs --serve --watch did not hot-reload on .rs edit");

    // 2. Direct script watch: `zipp rs --watch script.rs` writes marker file on every edit.
    let marker = tmp_dir.join("marker.txt");
    let script_rs = tmp_dir.join("script.rs");
    let s1 = format!(
        "fn main() {{ std::fs::write({:?}, \"run-v1\").unwrap(); }}\n",
        marker.to_str().unwrap()
    );
    let s2 = format!(
        "fn main() {{ std::fs::write({:?}, \"run-v2\").unwrap(); }}\n",
        marker.to_str().unwrap()
    );
    std::fs::write(&script_rs, &s1).unwrap();

    let mut watcher = Command::new(exe)
        .args(["rs", "--watch", script_rs.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zipp rs --watch");

    let deadline1 = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline1 {
        if std::fs::read_to_string(&marker).ok().as_deref() == Some("run-v1") {
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    assert_eq!(
        std::fs::read_to_string(&marker).ok().as_deref(),
        Some("run-v1"),
        "initial zipp rs --watch run did not execute"
    );

    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(&script_rs, &s2).unwrap();

    let deadline2 = Instant::now() + Duration::from_secs(10);
    let mut reloaded = false;
    while Instant::now() < deadline2 {
        if std::fs::read_to_string(&marker).ok().as_deref() == Some("run-v2") {
            reloaded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = watcher.kill();
    let _ = watcher.wait();
    let _ = std::fs::remove_dir_all(&tmp_dir);
    assert!(reloaded, "zipp rs --watch did not re-run script after .rs edit");
}
