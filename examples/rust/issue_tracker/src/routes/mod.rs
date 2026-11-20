use std::sync::{Arc, Mutex};
use std::time::Instant;
use http::{Request, Response, Router};
use serde_json::json;

use crate::config::AppConfig;
use crate::store::IssueStore;

pub mod auth;
pub mod issues;

pub struct AppState {
    pub config: AppConfig,
    pub store: Arc<Mutex<IssueStore>>,
}

async fn health(_req: Request, state: Arc<AppState>) -> Response {
    let stats = state.store.lock().unwrap().stats();
    Response::json(json!({
        "ok": true,
        "service": state.config.app_name,
        "version": state.config.app_version,
        "runtime": "ZIPP-Rust",
        "issues": stats.total,
        "users": stats.users,
    }))
}

pub fn build_router(config: AppConfig) -> Router {
    let store = IssueStore::open(&config.data_file)
        .expect("failed to initialize issue store");
    let state = Arc::new(AppState {
        config,
        store: Arc::new(Mutex::new(store)),
    });

    Router::new()
        .layer(|req: Request, next| {
            let started = Instant::now();
            let req_id = crypto::uuid_v4();
            let resp = next(req);
            let elapsed_us = started.elapsed().as_micros();
            resp.with_header("X-Request-Id", &req_id)
                .with_header("X-Runtime", "ZIPP-Rust")
                .with_header("X-Response-Time-Us", &elapsed_us.to_string())
        })
        .get("/api/health", health)
        .nest("/api/auth", auth::routes())
        .nest("/api/issues", issues::routes())
        .serve_dir("/static", "public")
        .serve_file("/", "public/index.html")
        .with_state(state)
}
