use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub github: GithubConfig,
    #[serde(default)]
    pub apps: Vec<AppConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub bind: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GithubConfig {
    /// TTL in seconds for the releases list cache.
    pub cache_ttl_seconds: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// URL-safe identifier used in API paths.
    pub slug: String,
    /// GitHub repository in "owner/repo" format.
    pub github_repo: String,
    pub display_name: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".to_string(),
        }
    }
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            cache_ttl_seconds: 60,
        }
    }
}

/// Load config from `path`, falling back to built-in defaults when the file
/// is absent.
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        anyhow::bail!(
            "Config file not found at {}. Copy config.example.toml to config.toml.",
            path.display()
        );
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;

    toml::from_str(&raw).with_context(|| "parsing config file")
}

impl Config {
    /// Find an app by slug. Returns `None` when not found.
    pub fn find_app(&self, slug: &str) -> Option<&AppConfig> {
        self.apps.iter().find(|a| a.slug == slug)
    }
}
