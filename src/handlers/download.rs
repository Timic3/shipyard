use crate::{error::AppError, manifest::Platform, AppState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
};

/// GET /v1/download/:app/:version/:platform
///
/// Resolves the binary asset for the requested release and returns a 302
/// redirect to a GitHub-signed download URL. No binary data is proxied.
pub async fn download(
    State(state): State<AppState>,
    Path((app_slug, version, platform_str)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = state
        .config
        .find_app(&app_slug)
        .ok_or(AppError::AppNotFound)?;

    let platform = Platform::parse(&platform_str).ok_or(AppError::InvalidPlatform)?;

    let asset_api_url = crate::handlers::update::resolve_asset_url(
        &state.github,
        &app.github_repo,
        &version,
        platform,
    )
    .await?;

    // Ask GitHub for the signed redirect URL.
    let signed_url = state.github.asset_redirect_url(&asset_api_url).await?;

    Ok((StatusCode::FOUND, [(header::LOCATION, signed_url)]))
}
