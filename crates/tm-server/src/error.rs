use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("policy error: {0}")]
    Policy(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("backend error: {0}")]
    Backend(String),
}

impl From<serde_json::Error> for ServerError {
    fn from(err: serde_json::Error) -> Self {
        Self::InvalidRequest(err.to_string())
    }
}

pub type Result<T, E = ServerError> = std::result::Result<T, E>;

impl IntoResponse for ServerError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            ServerError::NotFound(_) => StatusCode::NOT_FOUND,
            ServerError::Unauthorized => StatusCode::UNAUTHORIZED,
            ServerError::Forbidden | ServerError::Policy(_) => StatusCode::FORBIDDEN,
            ServerError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ServerError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ServerError::Backend(_) => StatusCode::BAD_GATEWAY,
        };
        let body = Json(json!({ "error": self.to_string() }));
        (status, body).into_response()
    }
}
