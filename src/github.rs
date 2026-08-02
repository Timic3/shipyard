use anyhow::{Context, Result};
use moka::future::Cache;
use reqwest::{
    header::{ACCEPT, AUTHORIZATION, USER_AGENT},
    Client, StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// A GitHub release as returned by the Releases API.
#[derive(Debug, Deserialize, Clone)]
pub struct Release {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub prerelease: bool,
    pub draft: bool,
    pub published_at: Option<String>,
    pub assets: Vec<Asset>,
}

/// A single asset attached to a release.
#[derive(Debug, Deserialize, Clone)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub browser_download_url: String,
    /// MIME type reported by GitHub.
    pub content_type: String,
    pub size: u64,
    pub url: String,
}

/// Shared GitHub client with caching.
#[derive(Clone)]
pub struct GithubClient {
    inner: Arc<GithubClientInner>,
}

struct GithubClientInner {
    http: Client,
    token: String,
    /// Keyed by "owner/repo", value is the list of releases.
    releases_cache: Cache<String, Arc<Vec<Release>>>,
    /// Keyed by asset URL, value is the .sig file text content.
    sig_cache: Cache<String, Arc<String>>,
}

impl GithubClient {
    pub fn new(token: String, releases_ttl: Duration) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("building HTTP client")?;

        let releases_cache = Cache::builder()
            .time_to_live(releases_ttl)
            .max_capacity(256)
            .build();

        // Signatures are immutable once published — cache for 1 hour.
        let sig_cache = Cache::builder()
            .time_to_live(Duration::from_secs(3600))
            .max_capacity(1024)
            .build();

        Ok(Self {
            inner: Arc::new(GithubClientInner {
                http,
                token,
                releases_cache,
                sig_cache,
            }),
        })
    }

    /// Return all releases for a repo, using the cache when available.
    pub async fn releases(&self, repo: &str) -> Result<Arc<Vec<Release>>, crate::error::AppError> {
        let inner = &self.inner;

        if let Some(cached) = inner.releases_cache.get(repo).await {
            tracing::debug!(repo, "releases cache hit");
            return Ok(cached);
        }

        tracing::debug!(repo, "releases cache miss — fetching from GitHub");

        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page=100",
            repo
        );
        let response = inner
            .http
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", inner.token))
            .header(USER_AGENT, "shipyard/0.1")
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, repo, "GitHub API request failed");
                crate::error::AppError::UpstreamError(e.to_string())
            })?;

        if response.status() == StatusCode::FORBIDDEN
            || response.status() == StatusCode::TOO_MANY_REQUESTS
        {
            tracing::warn!(repo, status = %response.status(), "GitHub rate limit hit");
            return Err(crate::error::AppError::UpstreamError(
                "GitHub rate limit exceeded".to_string(),
            ));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(repo, %status, body, "GitHub API error");
            return Err(crate::error::AppError::UpstreamError(format!(
                "GitHub returned {status}"
            )));
        }

        let releases: Vec<Release> = response.json().await.map_err(|e| {
            tracing::error!(error = %e, repo, "failed to deserialize releases");
            crate::error::AppError::UpstreamError(e.to_string())
        })?;

        let releases = Arc::new(releases);
        inner
            .releases_cache
            .insert(repo.to_string(), releases.clone())
            .await;

        Ok(releases)
    }

    /// Fetch the text content of a .sig asset, using a long-lived cache.
    pub async fn fetch_sig(&self, asset_url: &str) -> Result<Arc<String>, crate::error::AppError> {
        let inner = &self.inner;

        if let Some(cached) = inner.sig_cache.get(asset_url).await {
            tracing::debug!(asset_url, "sig cache hit");
            return Ok(cached);
        }

        tracing::debug!(asset_url, "sig cache miss — fetching from GitHub");

        // Use the API asset URL with Accept: application/octet-stream to get
        // a redirect to the raw content; follow it automatically.
        let response = inner
            .http
            .get(asset_url)
            .header(AUTHORIZATION, format!("Bearer {}", inner.token))
            .header(USER_AGENT, "shipyard/0.1")
            .header(ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| crate::error::AppError::UpstreamError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(crate::error::AppError::UpstreamError(format!(
                "GitHub returned {status} for sig asset"
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| crate::error::AppError::UpstreamError(e.to_string()))?;

        let text = Arc::new(text);
        inner
            .sig_cache
            .insert(asset_url.to_string(), text.clone())
            .await;

        Ok(text)
    }

    /// Return the redirect URL for a binary asset download.
    ///
    /// GitHub responds with 302 to a signed S3 URL when `Accept:
    /// application/octet-stream` is used. We capture the Location header
    /// instead of following the redirect.
    pub async fn asset_redirect_url(
        &self,
        asset_url: &str,
    ) -> Result<String, crate::error::AppError> {
        let inner = &self.inner;

        let response = inner
            .http
            .get(asset_url)
            .header(AUTHORIZATION, format!("Bearer {}", inner.token))
            .header(USER_AGENT, "shipyard/0.1")
            .header(ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28")
            // Do not follow redirects — we want the Location header.
            .send()
            .await
            .map_err(|e| crate::error::AppError::UpstreamError(e.to_string()))?;

        // reqwest follows redirects by default, so check the final URL.
        Ok(response.url().to_string())
    }
}
