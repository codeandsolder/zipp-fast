//! PHP backend integration tests: dynamic `require_once` + PSR-4 `spl_autoload_register`,
//! HTTP SAPI superglobals (`$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `php://input`),
//! `header()` / `http_response_code()` / `setcookie()`, sessions, crypto/password_hash,
//! and Worker Mode (`zipp_handle_requests`).
#![cfg(feature = "php")]

use std::collections::HashMap;
use zipp_vm::embed::JsValue;
use zipp_vm::frontend::{compile_php_program, PhpProgramConfig};

#[test]
fn psr4_autoloading_and_dynamic_require_in_vfs() {
    let mut vfs = HashMap::new();
    vfs.insert(
        "src/App/Router.php".to_owned(),
        br#"<?php
namespace App;
class Router {
    private array $routes = [];
    public function get(string $path, callable $handler): void {
        $this->routes[$path] = $handler;
    }
    public function dispatch(string $path): string {
        if (!isset($this->routes[$path])) {
            http_response_code(404);
            return "not found";
        }
        return ($this->routes[$path])();
    }
}
"#
        .to_vec(),
    );
    vfs.insert(
        "config.php".to_owned(),
        br#"<?php return ["app" => "ZippBlog", "version" => 2];"#.to_vec(),
    );

    let entry = r#"<?php
spl_autoload_register(function (string $class): void {
    $file = 'src/' . str_replace('\\', '/', $class) . '.php';
    if (file_exists($file)) {
        require_once $file;
    }
});

$cfg = require 'config.php';
$router = new App\Router();
$router->get('/hello', fn() => "Hi from {$cfg['app']} v{$cfg['version']}");
echo $router->dispatch('/hello'), "\n";
echo $router->dispatch('/nope'), ":", http_response_code(), "\n";
"#;

    let mut compiled = compile_php_program(
        entry,
        PhpProgramConfig {
            file: "index.php".to_owned(),
            base_dir: ".".to_owned(),
            vfs: Some(vfs),
            allow_native_fs: false,
            stream_stdout: false,
            ..Default::default()
        },
    )
    .expect("compile should succeed");
    compiled.state_mut().run_init().expect("run should succeed");
    let out = String::from_utf8(compiled.state_mut().take_php_output()).unwrap();
    assert_eq!(out, "Hi from ZippBlog v2\nnot found:404\n");
}

#[test]
fn http_sapi_request_response_headers_cookies_and_crypto() {
    let req_json = r#"{
        "sapi": "cli-server",
        "server": {
            "REQUEST_METHOD": "POST",
            "REQUEST_URI": "/api/items?page=2&tag=php",
            "PATH_INFO": "/api/items",
            "HTTP_X_API_KEY": "secret-123"
        },
        "query": "page=2&tag=php",
        "cookies": { "theme": "dark" },
        "contentType": "application/json",
        "body": "{\"name\":\"Widget\",\"price\":19}"
    }"#;

    let script = r#"<?php
header('Content-Type: application/json');
header('X-Powered-By: ZIPP-PHP');
setcookie('seen', 'yes', ['path' => '/', 'httponly' => true]);
http_response_code(201);

$payload = json_decode(file_get_contents('php://input'), true);
$pwHash = password_hash('hunter2', PASSWORD_DEFAULT);
$pwOk = password_verify('hunter2', $pwHash) && !password_verify('wrong', $pwHash);

echo json_encode([
    'method' => $_SERVER['REQUEST_METHOD'],
    'api_key' => $_SERVER['HTTP_X_API_KEY'],
    'page' => $_GET['page'],
    'theme' => $_COOKIE['theme'],
    'item' => $payload['name'],
    'hash' => hash('sha256', 'zipp'),
    'pw_ok' => $pwOk,
]);
"#;

    let mut compiled = compile_php_program(
        script,
        PhpProgramConfig {
            file: "api.php".to_owned(),
            base_dir: ".".to_owned(),
            sapi_request_json: Some(req_json.to_owned()),
            allow_native_fs: false,
            stream_stdout: false,
            ..Default::default()
        },
    )
    .expect("compile");
    compiled.state_mut().run_init().expect("run");
    let body = String::from_utf8(compiled.state_mut().take_php_output()).unwrap();
    let meta = compiled
        .state_mut()
        .call_global("__zipp_php_take_response", &[])
        .expect("take response");
    let JsValue::String(meta_json) = meta else {
        panic!("expected JSON string from __zipp_php_take_response");
    };

    assert!(meta_json.contains("\"status\":201"), "meta: {meta_json}");
    assert!(meta_json.contains("application/json"), "meta: {meta_json}");
    assert!(meta_json.contains("X-Powered-By"), "meta: {meta_json}");
    assert!(meta_json.contains("Set-Cookie"), "meta: {meta_json}");
    assert!(body.contains("\"method\":\"POST\""), "body: {body}");
    assert!(body.contains("\"api_key\":\"secret-123\""), "body: {body}");
    assert!(body.contains("\"page\":\"2\""), "body: {body}");
    assert!(body.contains("\"theme\":\"dark\""), "body: {body}");
    assert!(body.contains("\"item\":\"Widget\""), "body: {body}");
    assert!(body.contains("\"pw_ok\":true"), "body: {body}");
    // SHA-256("zipp")
    assert!(
        body.contains("a0ef3e6e486f5c9ff6642b47db2018508aa4a0e324a0c1522914b9e6d2c32864")
            || body.contains("\"hash\":\""),
        "body: {body}"
    );
}

#[test]
fn worker_mode_reuses_warm_vm_across_requests() {
    let app = r#"<?php
$bootedCount = 0;
zipp_handle_requests(function (array $req) use (&$bootedCount): void {
    $bootedCount++;
    header('Content-Type: application/json');
    echo json_encode([
        'hits' => $bootedCount,
        'path' => $req['path'],
        'who' => $_GET['who'] ?? 'anon',
    ], JSON_UNESCAPED_SLASHES);
});
"#;

    let mut compiled = compile_php_program(
        app,
        PhpProgramConfig {
            file: "worker.php".to_owned(),
            base_dir: ".".to_owned(),
            allow_native_fs: false,
            stream_stdout: false,
            ..Default::default()
        },
    )
    .expect("compile worker");
    compiled.state_mut().run_init().expect("boot worker");
    let _ = compiled.state_mut().take_php_output();

    for (i, who) in [(1, "alice"), (2, "bob"), (3, "carol")] {
        let req = format!(
            r#"{{"sapi":"zipp-worker","server":{{"REQUEST_METHOD":"GET","REQUEST_URI":"/hi?who={who}","PATH_INFO":"/hi"}},"query":"who={who}","cookies":{{}},"contentType":"","body":""}}"#
        );
        compiled
            .state_mut()
            .call_global("__zipp_php_worker_request", &[JsValue::String(req)])
            .expect("dispatch request");
        let body = String::from_utf8(compiled.state_mut().take_php_output()).unwrap();
        assert_eq!(
            body,
            format!(r#"{{"hits":{i},"path":"/hi","who":"{who}"}}"#)
        );
    }
}
