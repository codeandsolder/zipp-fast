use std::sync::Arc;
use http::{Request, Response, Router};

use crate::auth::require_auth;
use crate::error::AppError;
use crate::models::{
    AddCommentRequest, CreateIssueRequest, IssueStatus, Priority, UpdateIssueRequest,
};
use crate::routes::AppState;

fn parse_id(req: &Request) -> Result<i64, AppError> {
    let raw = req
        .param("id")
        .ok_or_else(|| AppError::BadRequest(String::from("missing issue id")))?;
    raw.parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid issue id `{raw}`")))
}

async fn list_issues(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let status = match req.query("status") {
        Some(s) if !s.trim().is_empty() => Some(IssueStatus::parse(&s)?),
        _ => None,
    };
    let priority = match req.query("priority") {
        Some(p) if !p.trim().is_empty() => Some(Priority::parse(&p)?),
        _ => None,
    };
    let q = req.query("q");
    let store = state.store.lock().unwrap();
    let issues = store.list_issues(status, priority, q);
    Ok(Response::json(issues))
}

async fn stats(_req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let store = state.store.lock().unwrap();
    Ok(Response::json(store.stats()))
}

async fn create_issue(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let claims = require_auth(&req, &state.config)?;
    let body: CreateIssueRequest = req.json::<CreateIssueRequest>()?;
    let mut store = state.store.lock().unwrap();
    let issue = store.create_issue(&claims.username, body)?;
    Ok(Response::created(issue))
}

async fn get_issue(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let id = parse_id(&req)?;
    let store = state.store.lock().unwrap();
    let issue = store.get_issue(id)?;
    Ok(Response::json(issue))
}

async fn update_issue(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let _claims = require_auth(&req, &state.config)?;
    let id = parse_id(&req)?;
    let body: UpdateIssueRequest = req.json::<UpdateIssueRequest>()?;
    let mut store = state.store.lock().unwrap();
    let updated = store.update_issue(id, body)?;
    Ok(Response::json(updated))
}

async fn add_comment(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let claims = require_auth(&req, &state.config)?;
    let id = parse_id(&req)?;
    let body: AddCommentRequest = req.json::<AddCommentRequest>()?;
    let mut store = state.store.lock().unwrap();
    let comment = store.add_comment(id, &claims.username, body)?;
    Ok(Response::created(comment))
}

async fn delete_issue(req: Request, state: Arc<AppState>) -> Result<Response, AppError> {
    let _claims = require_auth(&req, &state.config)?;
    let id = parse_id(&req)?;
    let mut store = state.store.lock().unwrap();
    store.delete_issue(id)?;
    Ok(Response::no_content())
}

pub fn routes() -> Router {
    Router::new()
        .get("/", list_issues)
        .get("/stats", stats)
        .post("/", create_issue)
        .get("/:id", get_issue)
        .patch("/:id", update_issue)
        .post("/:id/comments", add_comment)
        .delete("/:id", delete_issue)
}
