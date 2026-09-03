use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

const RELEASES_URL: &str = "https://api.github.com/repos/godotengine/godot-builds/releases";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub version: String,
    pub url: String,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("godot-nvm/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .context("could not initialize HTTP client")
}

pub fn stable_versions(cache_dir: &Path, force_refresh: bool) -> Result<Vec<String>> {
    let cache = cache_dir.join("stable-versions.json");
    if !force_refresh && cache_is_fresh(&cache) {
        return read_cache(&cache);
    }

    let fetched = (|| -> Result<Vec<String>> {
        let client = client()?;
        let mut stable = Vec::new();
        for page in 1..=10 {
            let releases: Vec<GithubRelease> = client
                .get(RELEASES_URL)
                .query(&[("per_page", "100"), ("page", &page.to_string())])
                .send()?
                .error_for_status()?
                .json()
                .context("the official release response was malformed")?;
            let count = releases.len();
            stable.extend(
                releases
                    .into_iter()
                    .filter(|entry| {
                        !entry.draft && !entry.prerelease && entry.tag_name.ends_with("-stable")
                    })
                    .map(|entry| entry.tag_name),
            );
            if count < 100 {
                break;
            }
        }
        stable.dedup();
        if stable.is_empty() {
            bail!("the official version database contained no stable releases");
        }
        fs::create_dir_all(cache_dir)?;
        fs::write(&cache, serde_json::to_vec_pretty(&stable)?)?;
        Ok(stable)
    })();

    match fetched {
        Ok(versions) => Ok(versions),
        Err(network_error) if cache.exists() => read_cache(&cache).with_context(|| {
            format!("release refresh failed ({network_error:#}) and the cache was unreadable")
        }),
        Err(error) => Err(error).context("could not download the official Godot release list"),
    }
}

pub fn resolve_asset(version: &str) -> Result<ReleaseAsset> {
    if !version.ends_with("-stable")
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        bail!("{version:?} is not an official stable version tag");
    }
    let url =
        format!("https://api.github.com/repos/godotengine/godot-builds/releases/tags/{version}");
    let release: GithubRelease = client()?
        .get(url)
        .send()?
        .error_for_status()?
        .json()
        .context("the official release response was malformed")?;
    let suffixes: &[&str] = match std::env::consts::ARCH {
        "x86_64" => &["_linux.x86_64.zip", "_x11.64.zip"],
        "aarch64" => &["_linux.arm64.zip", "_linux.aarch64.zip"],
        other => bail!("official Godot builds are not supported on {other}"),
    };
    let asset = release
        .assets
        .into_iter()
        .find(|asset| {
            suffixes.iter().any(|suffix| asset.name.ends_with(suffix))
                && !asset.name.to_ascii_lowercase().contains("mono")
        })
        .with_context(|| {
            format!(
                "Godot {version} has no standard Linux build for {}",
                std::env::consts::ARCH
            )
        })?;
    Ok(ReleaseAsset {
        version: version.into(),
        url: asset.browser_download_url,
        filename: asset.name,
    })
}

fn cache_is_fresh(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < CACHE_TTL)
}

fn read_cache(path: &Path) -> Result<Vec<String>> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).context("cached release list is malformed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the GitHub API"]
    fn official_catalog_resolves_a_linux_editor() {
        let cache = tempfile::tempdir().unwrap();
        let versions = stable_versions(cache.path(), true).unwrap();
        let version = versions.first().expect("at least one stable Godot release");
        assert!(version.ends_with("-stable"));
        let asset = resolve_asset(version).unwrap();
        assert!(asset.filename.ends_with(".zip"));
        assert!(!asset.filename.to_ascii_lowercase().contains("mono"));
    }
}
