use http::Response;
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl AppError {
    pub fn status_code(&self) -> i32 {
        match self {
            AppError::BadRequest(_) => 400,
            AppError::Unauthorized(_) => 401,
            AppError::Forbidden(_) => 403,
            AppError::NotFound(_) => 404,
            AppError::Conflict(_) => 409,
            AppError::Internal(_) => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            AppError::BadRequest(m) => m.clone(),
            AppError::Unauthorized(m) => m.clone(),
            AppError::Forbidden(m) => m.clone(),
            AppError::NotFound(m) => m.clone(),
            AppError::Conflict(m) => m.clone(),
            AppError::Internal(m) => m.clone(),
        }
    }

    pub fn into_response(&self) -> Response {
        let payload = json!({
            "error": self.message(),
            "status": self.status_code(),
        });
        match self {
            AppError::BadRequest(_) => Response::bad_request(payload),
            AppError::Unauthorized(_) => Response::unauthorized(payload),
            AppError::Forbidden(_) => Response::forbidden(payload),
            AppError::NotFound(_) => Response::not_found(payload),
            AppError::Conflict(_) => Response::conflict(payload),
            AppError::Internal(_) => Response::internal_error(payload),
        }
    }
}

impl From<String> for AppError {
    fn from(msg: String) -> Self {
        AppError::BadRequest(msg)
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Internal(format!("I/O error: {}", err.to_string()))
    }
}
