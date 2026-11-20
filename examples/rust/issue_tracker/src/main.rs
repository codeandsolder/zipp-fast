use http::Router;
use issue_tracker::config::AppConfig;
use issue_tracker::create_app;

/// Entry point discovered automatically by `zipp rs --serve HOST:PORT examples/rust/issue_tracker`.
pub fn router() -> Router {
    create_app()
}

/// Standalone execution (`zipp rs examples/rust/issue_tracker`): starts the HTTP server directly.
fn main() {
    let cfg = AppConfig::from_env();
    let addr = cfg.bind_addr.clone();
    println!(
        "{} v{} listening on http://{}",
        cfg.app_name, cfg.app_version, addr
    );
    let app = create_app();
    app.serve(&addr).expect("HTTP server failed");
}
