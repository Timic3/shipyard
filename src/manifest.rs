use crate::github::{Asset, Release};
use semver::Version;
use serde::Serialize;
use std::collections::HashMap;

/// Tauri v2 updater manifest (single-platform subset returned per request).
#[derive(Debug, Serialize)]
pub struct UpdateManifest {
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub platforms: HashMap<String, PlatformEntry>,
}

#[derive(Debug, Serialize)]
pub struct PlatformEntry {
    pub signature: String,
    pub url: String,
}

/// Recognized update channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stable" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            _ => None,
        }
    }
}

/// Recognized target platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    WindowsX86_64,
    DarwinX86_64,
    DarwinAarch64,
    LinuxX86_64,
}

impl Platform {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "windows-x86_64" => Some(Self::WindowsX86_64),
            "darwin-x86_64" => Some(Self::DarwinX86_64),
            "darwin-aarch64" => Some(Self::DarwinAarch64),
            "linux-x86_64" => Some(Self::LinuxX86_64),
            _ => None,
        }
    }

    /// Return the string form used in manifest platform keys and URL segments.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowsX86_64 => "windows-x86_64",
            Self::DarwinX86_64 => "darwin-x86_64",
            Self::DarwinAarch64 => "darwin-aarch64",
            Self::LinuxX86_64 => "linux-x86_64",
        }
    }

    /// Return true if `name` matches the binary asset pattern for this platform.
    pub fn matches_binary_asset(self, name: &str) -> bool {
        match self {
            Self::WindowsX86_64 => {
                (name.ends_with("_x64_en-US.msi") || name.ends_with("_x64-setup.exe"))
                    && !name.ends_with(".sig")
            }
            Self::DarwinX86_64 => name.ends_with("_x64.app.tar.gz") && !name.ends_with(".sig"),
            Self::DarwinAarch64 => name.ends_with("_aarch64.app.tar.gz") && !name.ends_with(".sig"),
            Self::LinuxX86_64 => {
                name.ends_with("_amd64.AppImage.tar.gz") && !name.ends_with(".sig")
            }
        }
    }

    /// Return true if `name` is the corresponding signature file.
    pub fn matches_sig_asset(self, name: &str) -> bool {
        match self {
            Self::WindowsX86_64 => {
                name.ends_with("_x64_en-US.msi.sig") || name.ends_with("_x64-setup.exe.sig")
            }
            Self::DarwinX86_64 => name.ends_with("_x64.app.tar.gz.sig"),
            Self::DarwinAarch64 => name.ends_with("_aarch64.app.tar.gz.sig"),
            Self::LinuxX86_64 => name.ends_with("_amd64.AppImage.tar.gz.sig"),
        }
    }
}

/// Parse a tag like "v1.2.3" or "1.2.3" into a `semver::Version`.
pub fn parse_tag(tag: &str) -> Option<Version> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(stripped).ok()
}

/// Filter `releases` according to `channel` and return the one with the
/// highest SemVer version, or `None` if no qualifying release exists.
pub fn latest_release(releases: &[Release], channel: Channel) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter(|r| match channel {
            // stable: non-pre-release only
            Channel::Stable => !r.prerelease,
            // beta: everything (pre-release and stable)
            Channel::Beta => true,
        })
        .filter_map(|r| parse_tag(&r.tag_name).map(|v| (v, r)))
        .max_by(|(va, _), (vb, _)| va.cmp(vb))
        .map(|(_, r)| r)
}

/// Find the binary asset and its corresponding .sig asset for `platform` in
/// the given release's asset list.
pub fn find_assets(release: &Release, platform: Platform) -> Option<(&Asset, &Asset)> {
    let binary = release
        .assets
        .iter()
        .find(|a| platform.matches_binary_asset(&a.name))?;
    let sig = release
        .assets
        .iter()
        .find(|a| platform.matches_sig_asset(&a.name))?;
    Some((binary, sig))
}

/// Build a `PlatformEntry` using an already-fetched signature string and the
/// Shipyard download URL.
pub fn build_platform_entry(signature: String, download_url: String) -> PlatformEntry {
    PlatformEntry {
        signature,
        url: download_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_release(tag: &str, prerelease: bool, draft: bool) -> Release {
        Release {
            tag_name: tag.to_string(),
            name: None,
            body: None,
            prerelease,
            draft,
            published_at: None,
            assets: vec![],
        }
    }

    #[test]
    fn parse_tag_strips_v_prefix() {
        assert_eq!(parse_tag("v1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(parse_tag("1.2.3"), Some(Version::new(1, 2, 3)));
        assert!(parse_tag("not-a-version").is_none());
    }

    #[test]
    fn latest_release_stable_skips_prerelease() {
        let releases = vec![
            make_release("v1.0.0", false, false),
            make_release("v1.1.0-beta.1", true, false),
            make_release("v0.9.0", false, false),
        ];
        let r = latest_release(&releases, Channel::Stable).unwrap();
        assert_eq!(r.tag_name, "v1.0.0");
    }

    #[test]
    fn latest_release_beta_includes_prerelease() {
        let releases = vec![
            make_release("v1.0.0", false, false),
            make_release("v1.1.0-beta.1", true, false),
        ];
        let r = latest_release(&releases, Channel::Beta).unwrap();
        assert_eq!(r.tag_name, "v1.1.0-beta.1");
    }

    #[test]
    fn latest_release_skips_drafts() {
        let releases = vec![
            make_release("v2.0.0", false, true), // draft
            make_release("v1.0.0", false, false),
        ];
        let r = latest_release(&releases, Channel::Stable).unwrap();
        assert_eq!(r.tag_name, "v1.0.0");
    }

    #[test]
    fn latest_release_none_when_empty() {
        assert!(latest_release(&[], Channel::Stable).is_none());
    }

    #[test]
    fn latest_release_respects_semver_ordering() {
        let releases = vec![
            make_release("v1.9.0", false, false),
            make_release("v1.10.0", false, false),
            make_release("v1.2.0", false, false),
        ];
        let r = latest_release(&releases, Channel::Stable).unwrap();
        assert_eq!(r.tag_name, "v1.10.0");
    }

    #[test]
    fn platform_matches_binary_windows() {
        assert!(Platform::WindowsX86_64.matches_binary_asset("myapp_1.0.0_x64_en-US.msi"));
        assert!(Platform::WindowsX86_64.matches_binary_asset("myapp_1.0.0_x64-setup.exe"));
        assert!(!Platform::WindowsX86_64.matches_binary_asset("myapp_1.0.0_x64_en-US.msi.sig"));
        assert!(!Platform::WindowsX86_64.matches_binary_asset("myapp_1.0.0_x64-setup.exe.sig"));
    }

    #[test]
    fn platform_matches_sig_linux() {
        assert!(Platform::LinuxX86_64.matches_sig_asset("myapp_1.0.0_amd64.AppImage.tar.gz.sig"));
        assert!(!Platform::LinuxX86_64.matches_sig_asset("myapp_1.0.0_amd64.AppImage.tar.gz"));
    }

    #[test]
    fn find_assets_returns_pair() {
        let mut release = make_release("v1.0.0", false, false);
        release.assets = vec![
            Asset {
                id: 1,
                name: "app_1.0.0_amd64.AppImage.tar.gz".to_string(),
                browser_download_url: "https://example.com/app.tar.gz".to_string(),
                content_type: "application/gzip".to_string(),
                size: 1000,
                url: "https://api.github.com/repos/owner/app/releases/assets/1".to_string(),
            },
            Asset {
                id: 2,
                name: "app_1.0.0_amd64.AppImage.tar.gz.sig".to_string(),
                browser_download_url: "https://example.com/app.tar.gz.sig".to_string(),
                content_type: "text/plain".to_string(),
                size: 100,
                url: "https://api.github.com/repos/owner/app/releases/assets/2".to_string(),
            },
        ];

        let (bin, sig) = find_assets(&release, Platform::LinuxX86_64).unwrap();
        assert_eq!(bin.name, "app_1.0.0_amd64.AppImage.tar.gz");
        assert_eq!(sig.name, "app_1.0.0_amd64.AppImage.tar.gz.sig");
    }

    #[test]
    fn find_assets_returns_none_when_missing() {
        let release = make_release("v1.0.0", false, false);
        assert!(find_assets(&release, Platform::LinuxX86_64).is_none());
    }
}
