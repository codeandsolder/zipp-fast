#![cfg(feature = "rust")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use zipp_vm::embed::JsValue;
use zipp_vm::frontend::{compile_rust_program, RustProgramConfig};

fn run_backend(src: &str, cfg: RustProgramConfig) -> String {
    let mut compiled = compile_rust_program(src, cfg).expect("rust compile");
    compiled.state_mut().run_init().expect("rust run");
    String::from_utf8(compiled.state_mut().take_rust_output()).expect("utf8")
}

#[test]
fn json_regex_crypto_time_and_env() {
    let src = r#"
        use serde_json::json;
        use regex::Regex;
        use std::time::{Instant, Duration};

        fn main() {
            let start = Instant::now();

            // JSON macro + serialization + struct deserialization
            #[derive(Debug)]
            struct User { id: i64, name: String }
            let raw = json!({ "id": 7, "name": "Alice", "roles": ["admin", "dev"] });
            let s = serde_json::to_string(&raw).unwrap();
            let parsed: User = serde_json::from_str::<User>(&s).unwrap();
            println!("json={} role0={}", parsed.name, raw["roles"][0].as_str().unwrap());

            // Regex
            let re = Regex::new(r"(\w+)@(\w+\.\w+)").unwrap();
            let caps = re.captures("contact: dev@zipp.org today").unwrap();
            println!("email={}@{}", caps.get(1).unwrap().as_str(), caps.get(2).unwrap().as_str());

            // Crypto (SHA-256, HMAC-SHA256, MD5, Base64, Hex, UUIDv4)
            let h = crypto::sha256("hello");
            let m = crypto::md5("hello");
            let b64 = crypto::base64_encode("zipp-rust");
            let dec = crypto::base64_decode(&b64);
            let uid = crypto::uuid_v4();
            println!("sha256={} md5={} b64={} dec={} uid_len={}", h, m, b64, dec, uid.len());

            // Env & Args
            std::env::set_var("ZIPP_RS_TEST", "ok123");
            let args: Vec<String> = std::env::args().collect();
            println!("env={} args={:?}", std::env::var("ZIPP_RS_TEST").unwrap(), args);

            // Duration & Instant
            let d = Duration::from_millis(250);
            assert_eq!(d.as_micros(), 250_000);
            assert!(start.elapsed().as_nanos() >= 0);
        }
    "#;
    let out = run_backend(
        src,
        RustProgramConfig {
            argv: vec!["app.rs".to_owned(), "--port".to_owned(), "8080".to_owned()],
            ..Default::default()
        },
    );
    assert!(out.contains("json=Alice role0=admin\n"), "{out}");
    assert!(out.contains("email=dev@zipp.org\n"), "{out}");
    assert!(
        out.contains("sha256=2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"),
        "{out}"
    );
    assert!(out.contains("md5=5d41402abc4b2a76b9719d911017c592"), "{out}");
    assert!(out.contains("b64=emlwcC1ydXN0 dec=zipp-rust uid_len=36"), "{out}");
    assert!(out.contains("env=ok123 args=[\"app.rs\", \"--port\", \"8080\"]"), "{out}");
}

#[test]
fn filesystem_and_vfs_sandbox_isolation() {
    // 1. Sandboxed VFS mode: cannot touch native FS, reads VFS cleanly
    let mut vfs = HashMap::new();
    vfs.insert("config.json".to_owned(), br#"{"env":"sandbox"}"#.to_vec());
    let out = run_backend(
        r#"
        fn main() {
            let txt = std::fs::read_to_string("config.json").unwrap();
            let v = serde_json::from_str::<serde_json::Value>(&txt).unwrap();
            println!("vfs={}", v["env"].as_str().unwrap());
            assert!(std::fs::write("blocked.txt", "nope").is_err());
        }
        "#,
        RustProgramConfig {
            vfs: Some(vfs),
            allow_native_fs: false,
            ..Default::default()
        },
    );
    assert_eq!(out, "vfs=sandbox\n");

    // 2. Native FS + PathBuf + KV store on a temp directory
    let tmp = std::env::temp_dir().join(format!("zipp_rs_fs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let out = run_backend(
        r#"
        use std::fs;
        use std::path::PathBuf;
        use zipp::kv::Store;

        fn main() {
            let p = PathBuf::from("notes.txt");
            fs::write(&p, "hello from rust vm").unwrap();
            let read = fs::read_to_string(&p).unwrap();
            let meta = fs::metadata(&p).unwrap();
            let kv = Store::open("state.json").unwrap();
            kv.set("user:1", "alice").unwrap();
            let kv2 = Store::open("state.json").unwrap();
            println!("read={read} len={} is_file={} kv={:?}",
                meta.len(), meta.is_file(), kv2.get("user:1"));
        }
        "#,
        RustProgramConfig {
            base_dir: tmp.to_string_lossy().replace('\\', "/"),
            allow_native_fs: true,
            ..Default::default()
        },
    );
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(
        out,
        "read=hello from rust vm len=18 is_file=true kv=Some(\"alice\")\n"
    );
}

#[test]
fn http_router_middleware_state_and_worker_dispatch() {
    let src = r#"
        use std::sync::{Arc, Mutex};
        use http::{Router, Request, Response};
        use serde_json::json;

        struct AppState {
            hits: i64,
        }

        fn router() -> Router {
            let state = Arc::new(Mutex::new(AppState { hits: 0 }));
            Router::new()
                .layer(|req: Request, next| {
                    let resp = next(req);
                    resp.with_header("X-Powered-By", "ZIPP-Rust")
                })
                .get("/health", |_req: Request, state: Arc<Mutex<AppState>>| {
                    let mut st = state.lock().unwrap();
                    st.hits += 1;
                    Response::json(json!({ "ok": true, "hits": st.hits }))
                })
                .get("/users/:id", |req: Request, _state| {
                    let id = req.param("id").unwrap();
                    let role = req.query("role").unwrap_or(String::from("guest"));
                    Response::json(json!({ "id": id, "role": role }))
                })
                .post("/echo", |req: Request, _state| {
                    let body = req.json::<serde_json::Value>().unwrap();
                    Response::json(json!({ "got": body["msg"] })).with_status(201)
                })
                .with_state(state)
        }
    "#;

    let mut compiled = compile_rust_program(
        src,
        RustProgramConfig {
            mode: "worker_boot".to_owned(),
            ..Default::default()
        },
    )
    .expect("compile router");
    compiled.state_mut().run_init().expect("boot router");

    let dispatch = |state: &mut zipp_vm::embed::ScriptState, method: &str, uri: &str, body: &str| {
        let req_json = format!(
            r#"{{"method":"{method}","uri":"{uri}","headers":{{}},"body":{:?}}}"#,
            body
        );
        match state
            .call_global("__zipp_rs_worker_request", &[JsValue::String(req_json)])
            .expect("dispatch")
        {
            JsValue::String(s) => s,
            other => panic!("unexpected response: {other:?}"),
        }
    };

    let r1 = dispatch(compiled.state_mut(), "GET", "/health", "");
    assert!(r1.contains(r#"\"hits\":1"#), "{r1}");
    assert!(r1.contains("X-Powered-By"), "{r1}");

    let r2 = dispatch(compiled.state_mut(), "GET", "/health", "");
    assert!(r2.contains(r#"\"hits\":2"#), "{r2}");

    let r3 = dispatch(compiled.state_mut(), "GET", "/users/42?role=admin", "");
    assert!(r3.contains(r#"\"id\":\"42\""#), "{r3}");
    assert!(r3.contains(r#"\"role\":\"admin\""#), "{r3}");

    let r4 = dispatch(compiled.state_mut(), "POST", "/echo", r#"{"msg":"hi"}"#);
    assert!(r4.contains("\"status\":201"), "{r4}");
    assert!(r4.contains(r#"\"got\":\"hi\""#), "{r4}");

    let r5 = dispatch(compiled.state_mut(), "GET", "/missing", "");
    assert!(r5.contains("\"status\":404"), "{r5}");
}

#[test]
fn native_tcp_networking_loopback() {
    // Spin up a tiny host TCP echo server on an ephemeral port and connect to it from Rust-on-ZIPP!
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 64];
            if let Ok(n) = s.read(&mut buf) {
                let mut reply = b"echo:".to_vec();
                reply.extend_from_slice(&buf[..n]);
                let _ = s.write_all(&reply);
            }
        }
    });

    let src = format!(
        r#"
        use std::net::TcpStream;

        fn main() {{
            let mut stream = TcpStream::connect("{addr}").unwrap();
            stream.write_all("hello-tcp").unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            println!("reply={{buf}}");
        }}
        "#
    );

    let out = run_backend(
        &src,
        RustProgramConfig {
            allow_net: true,
            ..Default::default()
        },
    );
    server.join().unwrap();
    assert_eq!(out, "reply=echo:hello-tcp\n");
}

#[test]
fn async_handlers_into_response_nested_routers_and_static_files() {
    let mut vfs = HashMap::new();
    vfs.insert(
        "public/index.html".to_owned(),
        b"<!doctype html><h1>Hello Rust App</h1>".to_vec(),
    );
    vfs.insert(
        "public/app.css".to_owned(),
        b"body { color: #222; }".to_vec(),
    );

    let src = r#"
        use http::{Router, Request, Response};
        use serde_json::json;

        enum ApiError {
            Unauthorized(String),
            BadRequest(String),
        }

        impl From<String> for ApiError {
            fn from(msg: String) -> Self {
                ApiError::BadRequest(msg)
            }
        }

        impl ApiError {
            fn into_response(&self) -> Response {
                match self {
                    ApiError::Unauthorized(m) => Response::unauthorized(json!({ "error": m })),
                    ApiError::BadRequest(m) => Response::bad_request(json!({ "error": m })),
                }
            }
        }

        async fn load_greeting(name: String) -> String {
            format!("hello, {name}")
        }

        async fn secret_handler(req: Request) -> Result<Response, ApiError> {
            let Some(tok) = req.bearer_token() else {
                return Err(ApiError::Unauthorized(String::from("missing bearer token")));
            };
            if tok != "valid-tok" {
                return Err(ApiError::Unauthorized(String::from("invalid token")));
            }
            let sid = req.cookie("sid").unwrap_or(String::from("none"));
            let greet = load_greeting(sid).await;
            Ok(Response::created(json!({ "greet": greet }))
                .with_cookie("seen", "1", "Path=/; HttpOnly"))
        }

        async fn form_handler(req: Request) -> Result<Response, ApiError> {
            let form = req.form();
            let user = form.get("user").cloned().ok_or(String::from("missing user"))?;
            Ok(Response::json(json!({ "user": user })))
        }

        fn router() -> Router {
            let api = Router::new()
                .get("/secret", secret_handler)
                .post("/form", form_handler);

            Router::new()
                .nest("/api/v1", api)
                .serve_dir("/static", "public")
                .serve_file("/", "public/index.html")
        }
    "#;

    let mut compiled = compile_rust_program(
        src,
        RustProgramConfig {
            vfs: Some(vfs),
            allow_native_fs: false,
            mode: "worker_boot".to_owned(),
            ..Default::default()
        },
    )
    .expect("compile router");
    compiled.state_mut().run_init().expect("boot router");

    let dispatch = |state: &mut zipp_vm::embed::ScriptState,
                    method: &str,
                    uri: &str,
                    headers: &str,
                    body: &str| {
        let req_json = format!(
            r#"{{"method":"{method}","uri":"{uri}","headers":{headers},"body":{:?}}}"#,
            body
        );
        match state
            .call_global("__zipp_rs_worker_request", &[JsValue::String(req_json)])
            .expect("dispatch")
        {
            JsValue::String(s) => s,
            other => panic!("unexpected response: {other:?}"),
        }
    };

    // Static index file & directory with MIME types
    let r_idx = dispatch(compiled.state_mut(), "GET", "/", "{}", "");
    assert!(r_idx.contains("\"status\":200"), "{r_idx}");
    assert!(r_idx.contains("text/html"), "{r_idx}");
    assert!(r_idx.contains("Hello Rust App"), "{r_idx}");

    let r_css = dispatch(compiled.state_mut(), "GET", "/static/app.css", "{}", "");
    assert!(r_css.contains("\"status\":200"), "{r_css}");
    assert!(r_css.contains("text/css"), "{r_css}");
    assert!(r_css.contains("body { color: #222; }"), "{r_css}");

    // Path traversal blocked
    let r_trav = dispatch(compiled.state_mut(), "GET", "/static/../Cargo.toml", "{}", "");
    assert!(
        r_trav.contains("\"status\":403") || r_trav.contains("\"status\":404"),
        "{r_trav}"
    );

    // Nested async route returning Err(ApiError) -> IntoResponse 401
    let r_unauth = dispatch(compiled.state_mut(), "GET", "/api/v1/secret", "{}", "");
    assert!(r_unauth.contains("\"status\":401"), "{r_unauth}");
    assert!(r_unauth.contains("missing bearer token"), "{r_unauth}");

    // Nested async route with Bearer + Cookie -> 201 Created + Set-Cookie
    let r_ok = dispatch(
        compiled.state_mut(),
        "GET",
        "/api/v1/secret",
        r#"{"authorization":"Bearer valid-tok","cookie":"sid=alice; theme=dark"}"#,
        "",
    );
    assert!(r_ok.contains("\"status\":201"), "{r_ok}");
    assert!(r_ok.contains(r#"\"greet\":\"hello, alice\""#), "{r_ok}");
    assert!(r_ok.contains("Set-Cookie"), "{r_ok}");

    // Form parsing + `?` From<String> -> ApiError::BadRequest (400)
    let r_form_err = dispatch(compiled.state_mut(), "POST", "/api/v1/form", "{}", "other=1");
    assert!(r_form_err.contains("\"status\":400"), "{r_form_err}");
    assert!(r_form_err.contains("missing user"), "{r_form_err}");

    let r_form_ok = dispatch(
        compiled.state_mut(),
        "POST",
        "/api/v1/form",
        "{}",
        "user=grace+hopper&role=adm",
    );
    assert!(r_form_ok.contains("\"status\":200"), "{r_form_ok}");
    assert!(r_form_ok.contains(r#"\"user\":\"grace hopper\""#), "{r_form_ok}");
}
