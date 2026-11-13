use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

/// Erreur d'API : un code stable (traduit côté navigateur) et un statut HTTP.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub const fn new(status: StatusCode, code: &'static str) -> Self { Self { status, code } }
    pub const fn bad(code: &'static str) -> Self { Self::new(StatusCode::BAD_REQUEST, code) }
    pub const fn unauthorized(code: &'static str) -> Self { Self::new(StatusCode::UNAUTHORIZED, code) }
    pub const fn forbidden(code: &'static str) -> Self { Self::new(StatusCode::FORBIDDEN, code) }
    pub const fn not_found() -> Self { Self::new(StatusCode::NOT_FOUND, "not_found") }
    pub const fn conflict(code: &'static str) -> Self { Self::new(StatusCode::CONFLICT, code) }
    pub const fn too_many() -> Self { Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited") }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("erreur interne : {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.code }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self { Self::internal(e) }
}
