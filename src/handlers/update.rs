use crate::{
    error::AppError,
    github::GithubClient,
    manifest::{
        build_platform_entry, find_assets, latest_release, parse_tag, Channel, Platform,
        UpdateManifest,
    },
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use semver::Version;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct UpdateQuery {
    pub current_version: Option<String>,
}

/// GET /v1/update/:app/:channel/:platform
pub async fn update(
    State(state): State<AppState>,
    Path((app_slug, channel_str, platform_str)): Path<(String, String, String)>,
    Query(query): Query<UpdateQuery>,
) -> Result<impl IntoResponse, AppError> {
    let app = state
        .config
        .find_app(&app_slug)
        .ok_or(AppError::AppNotFound)?;

    let channel = Channel::parse(&channel_str).ok_or(AppError::InvalidChannel)?;
    let platform = Platform::parse(&platform_str).ok_or(AppError::InvalidPlatform)?;

    let releases = state.github.releases(&app.github_repo).await?;

    let release = latest_release(&releases, channel).ok_or(AppError::NoUpdateAvailable)?;

    // If the client already has this version (or newer), no update needed.
    if let Some(current_str) = &query.current_version {
        if let (Some(current), Some(latest)) = (
            Version::parse(current_str).ok(),
            parse_tag(&release.tag_name),
        ) {
            if current >= latest {
                return Err(AppError::NoUpdateAvailable);
            }
        }
    }

    let (_binary_asset, sig_asset) =
        find_assets(release, platform).ok_or(AppError::NoUpdateAvailable)?;

    let signature = state
        .github
        .fetch_sig(&sig_asset.url)
        .await
        .map(|s| s.as_ref().clone())?;

    let version_str = parse_tag(&release.tag_name)
        .map(|v| v.to_string())
        .unwrap_or_else(|| release.tag_name.trim_start_matches('v').to_string());

    let download_url = format!(
        "{}/v1/download/{}/{}/{}",
        state.base_url,
        app_slug,
        version_str,
        platform.as_str()
    );

    let mut platforms = HashMap::new();
    platforms.insert(
        platform.as_str().to_string(),
        build_platform_entry(signature, download_url),
    );

    let manifest = UpdateManifest {
        version: version_str,
        notes: release.body.clone().unwrap_or_default(),
        pub_date: release
            .published_at
            .clone()
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()),
        platforms,
    };

    Ok(Json(manifest))
}

// ---------------------------------------------------------------------------
// Helper used by the download handler to find a specific versioned release.
// ---------------------------------------------------------------------------

/// Find the release in `releases` whose tag matches `version` (with or without
/// a leading `v`), or `None` when absent.
pub fn find_release_by_version<'a>(
    releases: &'a [crate::github::Release],
    version: &str,
) -> Option<&'a crate::github::Release> {
    let target = Version::parse(version).ok()?;
    releases
        .iter()
        .find(|r| parse_tag(&r.tag_name).map(|v| v == target).unwrap_or(false))
}

/// Find a release by version and resolve the binary asset URL for `platform`.
pub async fn resolve_asset_url(
    github: &GithubClient,
    repo: &str,
    version: &str,
    platform: Platform,
) -> Result<String, AppError> {
    let releases = github.releases(repo).await?;
    let release = find_release_by_version(&releases, version).ok_or(AppError::NoUpdateAvailable)?;
    let (binary, _sig) = find_assets(release, platform).ok_or(AppError::NoUpdateAvailable)?;
    Ok(binary.url.clone())
}
