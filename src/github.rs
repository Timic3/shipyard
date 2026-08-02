use anyhow::{Context, Result};
use moka::future::Cache;
use reqwest::{
    header::{ACCEPT, AUTHORIZATION, LOCATION, USER_AGENT},
    redirect::Policy,
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
    /// Client that never follows redirects, used to read asset `Location`
    /// headers without ever issuing a request to the CDN.
    no_redirect: Client,
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

        let no_redirect = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(Policy::none())
            .build()
            .context("building no-redirect HTTP client")?;

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
                no_redirect,
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
    /// GitHub responds with 302 to a short-lived signed CDN URL when `Accept:
    /// application/octet-stream` is used. We read the Location header off that
    /// 302 rather than following it: the token is only ever sent to
    /// api.github.com, and we never open a connection to the CDN ourselves.
    pub async fn asset_redirect_url(
        &self,
        asset_url: &str,
    ) -> Result<String, crate::error::AppError> {
        let inner = &self.inner;

        let response = inner
            .no_redirect
            .get(asset_url)
            .header(AUTHORIZATION, format!("Bearer {}", inner.token))
            .header(USER_AGENT, "shipyard/0.1")
            .header(ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| crate::error::AppError::UpstreamError(e.to_string()))?;

        let status = response.status();
        if !status.is_redirection() {
            // Anything else (404, 403 rate limit, 200 with JSON metadata) must
            // not be handed to the client as a redirect target.
            tracing::warn!(%status, "GitHub did not redirect for asset download");
            return Err(crate::error::AppError::UpstreamError(format!(
                "expected redirect for asset, got {status}"
            )));
        }

        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                crate::error::AppError::UpstreamError(
                    "asset redirect had no usable Location header".to_string(),
                )
            })?;

        Ok(location.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Requests observed by one of the stub servers, as raw request heads.
    type Seen = Arc<Mutex<Vec<String>>>;

    async fn read_head(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// Stub server: `/asset` 302s to `redirect_to`, anything else returns the
    /// signature body. Every request head is recorded into `seen`.
    fn spawn_server(listener: TcpListener, redirect_to: String, seen: Seen) {
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_head(&mut stream).await;
                seen.lock().unwrap().push(head.clone());

                let response = if head.starts_with("GET /asset") {
                    format!("HTTP/1.1 302 Found\r\nLocation: {redirect_to}\r\nContent-Length: 0\r\n\r\n")
                } else {
                    let body = "SIGNATURE";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.flush().await.unwrap();
            }
        });
    }

    fn has_auth(head: &str) -> bool {
        head.to_lowercase().contains("authorization:")
    }

    fn client() -> GithubClient {
        GithubClient::new("SUPER-SECRET-TOKEN".to_string(), Duration::from_secs(60)).unwrap()
    }

    /// The token must reach the API host but never survive the cross-host
    /// redirect to the asset CDN. Both stubs listen on the same port so the
    /// only difference reqwest sees is the hostname — exactly the GitHub case.
    #[tokio::test]
    async fn fetch_sig_does_not_forward_token_across_hosts() {
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = api.local_addr().unwrap().port();
        let cdn = TcpListener::bind(format!("127.0.0.2:{port}")).await.unwrap();

        let api_seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let cdn_seen: Seen = Arc::new(Mutex::new(Vec::new()));

        spawn_server(
            api,
            format!("http://127.0.0.2:{port}/sink"),
            api_seen.clone(),
        );
        spawn_server(cdn, String::new(), cdn_seen.clone());

        let sig = client()
            .fetch_sig(&format!("http://127.0.0.1:{port}/asset"))
            .await
            .unwrap();

        assert_eq!(sig.as_str(), "SIGNATURE", "redirect should be followed");

        let api_head = api_seen.lock().unwrap()[0].clone();
        assert!(has_auth(&api_head), "token must be sent to the API host");

        let cdn_head = cdn_seen.lock().unwrap()[0].clone();
        assert!(
            !has_auth(&cdn_head),
            "token leaked to the CDN host: {cdn_head}"
        );
        assert!(
            !cdn_head.contains("SUPER-SECRET-TOKEN"),
            "token leaked to the CDN host: {cdn_head}"
        );
    }

    /// Control: on a same-origin redirect reqwest *does* forward the header,
    /// so the assertion above is capable of detecting a leak.
    #[tokio::test]
    async fn same_origin_redirect_still_forwards_token() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));

        spawn_server(listener, format!("http://127.0.0.1:{port}/sink"), seen.clone());

        client()
            .fetch_sig(&format!("http://127.0.0.1:{port}/asset"))
            .await
            .unwrap();

        let sink_head = seen.lock().unwrap()[1].clone();
        assert!(has_auth(&sink_head));
    }

    /// The download path must never open a connection to the CDN at all.
    #[tokio::test]
    async fn asset_redirect_url_does_not_contact_cdn() {
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = api.local_addr().unwrap().port();
        let cdn = TcpListener::bind(format!("127.0.0.2:{port}")).await.unwrap();

        let api_seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let cdn_seen: Seen = Arc::new(Mutex::new(Vec::new()));

        let signed = format!("http://127.0.0.2:{port}/sink?sig=abc");
        spawn_server(api, signed.clone(), api_seen.clone());
        spawn_server(cdn, String::new(), cdn_seen.clone());

        let location = client()
            .asset_redirect_url(&format!("http://127.0.0.1:{port}/asset"))
            .await
            .unwrap();

        assert_eq!(location, signed, "Location header should be returned as-is");
        assert!(!location.contains("SUPER-SECRET-TOKEN"));
        assert!(
            cdn_seen.lock().unwrap().is_empty(),
            "shipyard should never request the CDN itself"
        );
    }

    /// A non-redirect response must not be handed to the client as a target.
    #[tokio::test]
    async fn asset_redirect_url_rejects_non_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));

        // "/sink" path returns 200, not a redirect.
        spawn_server(listener, String::new(), seen);

        let result = client()
            .asset_redirect_url(&format!("http://127.0.0.1:{port}/sink"))
            .await;

        assert!(matches!(
            result,
            Err(crate::error::AppError::UpstreamError(_))
        ));
    }
}
