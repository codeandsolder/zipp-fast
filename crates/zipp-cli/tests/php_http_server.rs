//! End-to-end TCP HTTP test for `zipp php -S`.
#![cfg(feature = "php")]

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
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to zipp php server");
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp
}

#[test]
fn php_http_server_serves_templates_json_api_and_404() {
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_zipp");
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let backend_dir = repo_root.join("examples/php/backend");
    let router = backend_dir.join("index.php");

    let mut child = Command::new(exe)
        .args([
            "php",
            "-S",
            &format!("127.0.0.1:{port}"),
            "-t",
            backend_dir.to_str().unwrap(),
            router.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zipp php -S");

    // Wait for server socket to start listening
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // 1. GET / -> HTML template rendered via extract() + ob_start() + include
    let r1 = http_send(port, "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(r1.starts_with("HTTP/1.1 200 OK"), "r1:\n{r1}");
    assert!(r1.contains("<h1>ZIPP PHP Backend</h1>"), "r1:\n{r1}");
    assert!(r1.contains("In-process OPcache"), "r1:\n{r1}");

    // 2. GET /api/users/42?role=admin -> PCRE route params + $_GET + JSON
    let r2 = http_send(
        port,
        "GET /api/users/42?role=admin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(r2.starts_with("HTTP/1.1 200 OK"), "r2:\n{r2}");
    assert!(r2.contains("application/json"), "r2:\n{r2}");
    assert!(r2.contains("\"id\":42"), "r2:\n{r2}");
    assert!(r2.contains("\"role\":\"admin\""), "r2:\n{r2}");

    // 3. POST /api/echo -> 201 Created + php://input JSON + SHA-256
    let payload = r#"{"msg":"hello-zipp"}"#;
    let r3 = http_send(
        port,
        &format!(
            "POST /api/echo HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        ),
    );
    assert!(r3.starts_with("HTTP/1.1 201 Created"), "r3:\n{r3}");
    assert!(r3.contains("\"msg\":\"hello-zipp\""), "r3:\n{r3}");
    assert!(r3.contains("\"hash\":\""), "r3:\n{r3}");

    // 4. GET /missing -> 404 JSON
    let r4 = http_send(
        port,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(r4.starts_with("HTTP/1.1 404 Not Found"), "r4:\n{r4}");
    assert!(r4.contains("\"error\":\"Not Found\""), "r4:\n{r4}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn php_worker_http_server_handles_requests_on_warm_vm() {
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_zipp");
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let backend_dir = repo_root.join("examples/php/backend");
    let worker = backend_dir.join("worker.php");

    let mut child = Command::new(exe)
        .args([
            "php",
            "--worker",
            "-S",
            &format!("127.0.0.1:{port}"),
            "-t",
            backend_dir.to_str().unwrap(),
            worker.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zipp php --worker -S");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let r1 = http_send(port, "GET /api/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(r1.starts_with("HTTP/1.1 200 OK"), "r1:\n{r1}");
    assert!(r1.contains("\"pong\":true"), "r1:\n{r1}");

    let r2 = http_send(port, "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(r2.starts_with("HTTP/1.1 200 OK"), "r2:\n{r2}");
    assert!(r2.contains("\"mode\":\"worker\""), "r2:\n{r2}");

    let _ = child.kill();
    let _ = child.wait();
}
