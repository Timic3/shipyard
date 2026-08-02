use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("app not found")]
    AppNotFound,

    #[error("invalid channel")]
    InvalidChannel,

    #[error("invalid platform")]
    InvalidPlatform,

    #[error("no update available")]
    NoUpdateAvailable,

    #[error("upstream error: {0}")]
    UpstreamError(String),

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::AppNotFound => (StatusCode::NOT_FOUND, "app not found"),
            AppError::InvalidChannel => (StatusCode::BAD_REQUEST, "invalid channel"),
            AppError::InvalidPlatform => (StatusCode::BAD_REQUEST, "invalid platform"),
            AppError::NoUpdateAvailable => {
                return StatusCode::NO_CONTENT.into_response();
            }
            AppError::UpstreamError(_) => (StatusCode::BAD_GATEWAY, "upstream error"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };

        tracing::error!(error = %self, "request error");
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::UpstreamError(e.to_string())
    }
}
