use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

/// GET /health — liveness check.
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}
