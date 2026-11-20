use std::sync::Arc;
use http::{Request, Response, Router};

use crate::auth::{issue_token, require_auth};
use crate::error::AppError;
use crate::models::{AuthResponse, LoginRequest, RegisterRequest, User};
use crate::routes::AppState;

async fn register(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let body: RegisterRequest = req.json::<RegisterRequest>()?;
    let mut store = state.store.lock().unwrap();
    let rec = store.register_user(&body.username, &body.password)?;
    let token = issue_token(&rec, &state.config)?;
    let user: User = rec.into();
    Ok(Response::created(AuthResponse { token, user }))
}

async fn login(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let body: LoginRequest = req.json::<LoginRequest>()?;
    let store = state.store.lock().unwrap();
    let rec = store.authenticate(&body.username, &body.password)?;
    let token = issue_token(&rec, &state.config)?;
    let user: User = rec.into();
    Ok(Response::json(AuthResponse { token, user }))
}

async fn me(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let claims = require_auth(&req, &state.config)?;
    let store = state.store.lock().unwrap();
    let Some(user) = store.find_user_by_id(claims.user_id) else {
        return Err(AppError::NotFound(String::from("user no longer exists")));
    };
    Ok(Response::json(user))
}

pub fn routes() -> Router {
    Router::new()
        .post("/register", register)
        .post("/login", login)
        .get("/me", me)
}
