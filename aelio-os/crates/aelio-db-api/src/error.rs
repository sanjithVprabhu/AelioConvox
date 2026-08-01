//! API error type that renders as a JSON `{ "error": "..." }` body with an appropriate status.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    /// Malformed request (bad column kind, unknown op, missing field…).
    BadRequest(String),
    /// Missing/invalid API key.
    Unauthorized,
    /// Table/column/row not found.
    NotFound(String),
    /// A feature is not configured (e.g. NL queries with no LLM backend).
    Unavailable(String),
    /// A structurally valid request exceeded an explicit execution budget.
    ResourceLimit(String),
    /// An engine I/O error.
    Internal(String),
}

impl AppError {
    fn parts(&self) -> (StatusCode, &str) {
        match self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "missing or invalid API key"),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
            AppError::ResourceLimit(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            AppError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = self.parts();
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

/// Map an engine `io::Error` to an API error. The `Database` facade signals "no such
/// table/column" via `io::ErrorKind::Other` with a message, so we route those to 404 by
/// inspecting the message; everything else is a 500.
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        let m = e.to_string();
        if e.kind() == std::io::ErrorKind::InvalidInput {
            AppError::BadRequest(m)
        } else if m.starts_with("no such table") || m.starts_with("no such column") {
            AppError::NotFound(m)
        } else {
            AppError::Internal(m)
        }
    }
}
