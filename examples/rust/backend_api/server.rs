//! Multi-file Rust Backend HTTP/1.1 API server running on ZIPP!
//!
//! Run in multi-threaded worker mode:
//!   zipp rs --serve 127.0.0.1:8080 examples/rust/backend_api/server.rs
//!
//! Or directly as a standalone script:
//!   zipp rs examples/rust/backend_api/server.rs 127.0.0.1:8080

mod models;
mod routes;

use crate::models::AppState;
use http::{Request, Response, Router};
use std::sync::{Arc, Mutex};

pub fn router() -> Router {
    let state = Arc::new(Mutex::new(AppState::seeded()));
    Router::new()
        .layer(|req: Request, next| {
            let resp: Response = next(req);
            resp.with_header("X-Runtime", "ZIPP-Rust")
        })
        .get("/", |_req: Request, _state| {
            Response::html("<h1>ZIPP Rust Backend API</h1><p>Try GET /health or GET /api/items</p>")
        })
        .get("/health", routes::health)
        .get("/api/items", routes::list_items)
        .get("/api/items/:id", routes::get_item)
        .post("/api/items", routes::create_item)
        .with_state(state)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let addr = if args.len() > 1 {
        args[1].as_str()
    } else {
        "127.0.0.1:8080"
    };
    println!("Listening on http://{addr}");
    router().serve(addr).unwrap();
}
