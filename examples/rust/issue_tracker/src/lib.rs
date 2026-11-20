pub mod auth;
pub mod config;
pub mod error;
pub mod models;
pub mod routes;
pub mod store;

use http::Router;

pub fn create_app() -> Router {
    let cfg = config::AppConfig::from_env();
    routes::build_router(cfg)
}
