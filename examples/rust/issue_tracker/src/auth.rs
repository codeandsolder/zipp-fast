use crate::config::AppConfig;
use crate::error::AppError;
use crate::models::UserRecord;
use http::Request;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct AuthClaims {
    pub user_id: i64,
    pub username: String,
    pub role: String,
}

pub fn hash_password(password: &str) -> String {
    crypto::hash_password(password)
}

pub fn verify_password(password: &str, encoded: &str) -> bool {
    crypto::verify_password(password, encoded)
}

pub fn issue_token(user: &UserRecord, cfg: &AppConfig) -> Result<String, AppError> {
    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": user.id,
        "username": user.username,
        "role": user.role,
        "iat": now,
        "exp": now + 86400,
    });
    crypto::jwt_sign(&claims, &cfg.jwt_secret)
        .map_err(|e| AppError::Internal(format!("JWT signing error: {e}")))
}

pub fn verify_token(token: &str, cfg: &AppConfig) -> Result<AuthClaims, AppError> {
    let payload = crypto::jwt_verify(token, &cfg.jwt_secret)
        .map_err(|e| AppError::Unauthorized(format!("invalid or expired token: {e}")))?;
    let user_id = payload["sub"]
        .as_i64()
        .ok_or_else(|| AppError::Unauthorized(String::from("token missing `sub`")))?;
    let username = payload["username"]
        .as_str()
        .ok_or_else(|| AppError::Unauthorized(String::from("token missing `username`")))?
        .to_string();
    let role = payload["role"]
        .as_str()
        .unwrap_or("member")
        .to_string();
    Ok(AuthClaims {
        user_id,
        username,
        role,
    })
}

pub fn require_auth(req: &Request, cfg: &AppConfig) -> Result<AuthClaims, AppError> {
    let Some(token) = req.bearer_token() else {
        return Err(AppError::Unauthorized(String::from(
            "missing Authorization: Bearer <token> header",
        )));
    };
    verify_token(&token, cfg)
}
