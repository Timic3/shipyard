mod config;
mod error;
mod github;
mod handlers;
mod manifest;

use anyhow::{Context, Result};
use axum::{
    extract::MatchedPath,
    http::Request,
    middleware::{self, Next},
    response::Response,
    routing::get,
    Router,
};
use config::Config;
use github::GithubClient;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::info_span;

/// Shared application state injected into every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub github: GithubClient,
    /// Public base URL of this server (e.g. "https://shipyard.example.com").
    pub base_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialise structured logging from RUST_LOG env var.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "shipyard=info,tower_http=info".into()),
        )
        .init();

    // Fail fast if the GitHub token is absent.
    let github_token =
        std::env::var("GITHUB_TOKEN").context("GITHUB_TOKEN environment variable must be set")?;

    // Load config from the path given as the first CLI arg, defaulting to
    // "config.toml" in the current directory.
    let config_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string())
        .into();

    let config = config::load(&config_path)?;
    let bind_addr: SocketAddr = config
        .server
        .bind
        .parse()
        .with_context(|| format!("invalid bind address: {}", config.server.bind))?;

    let base_url = std::env::var("BASE_URL").unwrap_or_else(|_| format!("http://{}", bind_addr));

    tracing::info!(
        apps = config.apps.len(),
        bind = %bind_addr,
        %base_url,
        "starting shipyard"
    );

    let github = GithubClient::new(
        github_token,
        Duration::from_secs(config.github.cache_ttl_seconds),
    )?;

    let state = AppState {
        config: Arc::new(config),
        github,
        base_url,
    };

    let app = build_router(state);

    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding to {bind_addr}"))?;

    tracing::info!(%bind_addr, "listening");
    axum::serve(listener, app).await.context("server error")?;

    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health::health))
        .route(
            "/v1/update/:app/:channel/:platform",
            get(handlers::update::update),
        )
        .route(
            "/v1/download/:app/:version/:platform",
            get(handlers::download::download),
        )
        .layer(middleware::from_fn(request_logging_middleware))
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                let matched = req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|m| m.as_str())
                    .unwrap_or(req.uri().path());
                info_span!("http_request", method = %req.method(), path = matched)
            }),
        )
        .with_state(state)
}

async fn request_logging_middleware(req: Request<axum::body::Body>, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let start = std::time::Instant::now();

    let response = next.run(req).await;

    let elapsed = start.elapsed();
    tracing::info!(
        method = %method,
        path = %uri.path(),
        status = response.status().as_u16(),
        duration_ms = elapsed.as_millis(),
    );

    response
}
