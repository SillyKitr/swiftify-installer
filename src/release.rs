use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};

const LATEST_RELEASE: &str = "https://api.github.com/repos/swiftify-org/swiftify/releases/latest";
const PUBLIC_LATEST: &str = "https://github.com/swiftify-org/swiftify/releases/latest";
const CACHE_AGE: u64 = 15 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub asset_name: String,
    pub download_url: String,
    pub size: u64,
}

pub struct ReleaseCheck {
    pub release: Release,
    pub notice: Option<String>,
    pub retry_at: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    release: Option<Release>,
    etag: Option<String>,
    checked_at: u64,
    retry_at: Option<u64>,
    #[serde(default)]
    public_fallback: bool,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!(
            "swiftify-community-installer/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Could not initialize the network client: {e}"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn retry_pending(deadline: u64) -> bool {
    deadline > now()
}

pub fn saved_retry_at() -> Option<u64> {
    read_cache()
        .ok()
        .flatten()
        .and_then(|cache| cache.retry_at)
        .filter(|time| retry_pending(*time))
}

pub fn retry_message(deadline: u64) -> String {
    let minutes = deadline.saturating_sub(now()).div_ceil(60);
    format!("GitHub API rate limit: retry in about {minutes} minute(s).")
}

fn cache_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| {
            path.join("SwiftifyCommunityInstaller")
                .join("release-cache.json")
        })
}

fn read_cache() -> Result<Option<Cache>, String> {
    let Some(path) = cache_path() else {
        return Ok(None);
    };
    read_cache_at(&path)
}

fn read_cache_at(path: &std::path::Path) -> Result<Option<Cache>, String> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map(Some)
            .map_err(|e| format!("Saved release information is damaged: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Could not read saved release information: {e}")),
    }
}

fn save_cache(cache: &Cache) -> Result<(), String> {
    let path =
        cache_path().ok_or("LOCALAPPDATA is unavailable; release information cannot be saved.")?;
    save_cache_at(&path, cache)
}

fn save_cache_at(path: &std::path::Path, cache: &Cache) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("Invalid cache path.")?)
        .map_err(|e| format!("Could not create the release cache directory: {e}"))?;
    let temporary = path.with_file_name(format!(".release-{}.json", uuid::Uuid::new_v4()));
    let data = serde_json::to_vec(cache).map_err(|e| e.to_string())?;
    fs::write(&temporary, data).map_err(|e| format!("Could not save release information: {e}"))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(temporary);
        return Err(format!(
            "Could not replace saved release information: {error}"
        ));
    }
    Ok(())
}

fn exhausted_deadline(headers: &header::HeaderMap, current: u64) -> Option<u64> {
    (headers.get("x-ratelimit-remaining")?.to_str().ok()? == "0").then(|| {
        headers
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(current + CACHE_AGE)
            .max(current + 60)
    })
}

fn rate_limit_deadline(
    status: StatusCode,
    headers: &header::HeaderMap,
    current: u64,
) -> Option<u64> {
    if status != StatusCode::TOO_MANY_REQUESTS && status != StatusCode::FORBIDDEN {
        return None;
    }
    let retry_after = headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| current.saturating_add(seconds));
    let reset = exhausted_deadline(headers, current);
    Some(
        retry_after
            .into_iter()
            .chain(reset)
            .max()
            .unwrap_or(current + CACHE_AGE)
            .max(current + 60),
    )
}

async fn public_release(client: &reqwest::Client) -> Result<Release, String> {
    // The public latest redirect points to a stable tag; a HEAD on the versioned asset
    // confirms that it exists and supplies the exact byte count without downloading it.
    let response = tokio::time::timeout(Duration::from_secs(30), client.get(PUBLIC_LATEST).send())
        .await
        .map_err(|_| "The public releases page timed out.".to_owned())?
        .map_err(|e| format!("Could not reach public releases: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Public releases returned an error: {e}"))?;
    let url = response.url();
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err("Public latest release redirected away from GitHub.".into());
    }
    let prefix = "/swiftify-org/swiftify/releases/tag/";
    let tag = url
        .path()
        .strip_prefix(prefix)
        .ok_or("Public latest release did not resolve to a release tag.")?;
    if tag.is_empty()
        || !tag.starts_with('v')
        || !tag[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
        || !tag[1..].contains('.')
    {
        return Err("Public latest release has an unexpected version tag.".into());
    }
    let name = format!("swiftify-{}-windows-x86_64.zip", &tag[1..]);
    let download_url =
        format!("https://github.com/swiftify-org/swiftify/releases/download/{tag}/{name}");
    let asset = tokio::time::timeout(Duration::from_secs(30), client.head(&download_url).send())
        .await
        .map_err(|_| "Checking the public Windows archive timed out.".to_owned())?
        .map_err(|e| format!("Could not check the public Windows archive: {e}"))?
        .error_for_status()
        .map_err(|e| format!("The public Windows archive is unavailable: {e}"))?;
    if asset
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("text/html"))
    {
        return Err("The public archive URL returned a web page instead of an archive.".into());
    }
    // HEAD responses have no body; reqwest::Response::content_length may report zero
    // even though the server supplied the asset's Content-Length header.
    let size = asset
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|size| *size > 0)
        .ok_or("The public Windows archive did not provide a usable Content-Length.")?;
    Ok(Release {
        version: tag.into(),
        asset_name: name,
        download_url,
        size,
    })
}

pub async fn latest(client: &reqwest::Client, force: bool) -> Result<ReleaseCheck, String> {
    let cache_error;
    let mut cached = match read_cache() {
        Ok(cache) => {
            cache_error = None;
            cache
        }
        Err(error) => {
            cache_error = Some(error);
            None
        }
    };
    let current = now();
    let retry_at = cached
        .as_ref()
        .and_then(|cache| cache.retry_at)
        .filter(|time| *time > current);
    if !force
        && cached.as_ref().is_some_and(|cache| {
            cache.release.is_some()
                && current >= cache.checked_at
                && current - cache.checked_at < CACHE_AGE
        })
    {
        let cache = cached.unwrap();
        return Ok(ReleaseCheck {
            release: cache.release.unwrap(),
            notice: Some(if cache.public_fallback {
                format!(
                    "Using cached public release metadata. {}",
                    retry_at.map(retry_message).unwrap_or_default()
                )
            } else {
                format!(
                    "Using recently saved GitHub release metadata. {}",
                    retry_at.map(retry_message).unwrap_or_default()
                )
            }),
            retry_at,
        });
    }
    let mut failure = cache_error.unwrap_or_default();
    if let Some(deadline) = retry_at {
        failure = retry_message(deadline);
    } else {
        let mut request = client.get(LATEST_RELEASE);
        if let Some(etag) = cached.as_ref().and_then(|cache| cache.etag.as_ref()) {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        match tokio::time::timeout(Duration::from_secs(30), request.send()).await {
            Ok(Ok(response)) => {
                let status = response.status();
                if status == StatusCode::NOT_MODIFIED {
                    let deadline = exhausted_deadline(response.headers(), current);
                    if let Some(mut cache) = cached.take().filter(|cache| cache.release.is_some()) {
                        cache.checked_at = current;
                        cache.retry_at = deadline;
                        let notice = save_cache(&cache)
                            .err()
                            .or_else(|| deadline.map(retry_message));
                        return Ok(ReleaseCheck {
                            release: cache.release.unwrap(),
                            notice,
                            retry_at: deadline,
                        });
                    }
                    failure = "GitHub returned 304 without a saved release.".into();
                } else if status.is_success() {
                    let deadline = exhausted_deadline(response.headers(), current);
                    let etag = response
                        .headers()
                        .get(header::ETAG)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    let parsed = tokio::time::timeout(
                        Duration::from_secs(30),
                        response.json::<GitHubRelease>(),
                    )
                    .await;
                    let release = match parsed {
                        Ok(Ok(data)) => Some(select_release(data)?),
                        Ok(Err(error)) => {
                            failure = format!("Could not read GitHub release information: {error}");
                            None
                        }
                        Err(_) => {
                            failure = "Reading GitHub release information timed out.".into();
                            None
                        }
                    };
                    if let Some(release) = release {
                        let cache = Cache {
                            release: Some(release.clone()),
                            etag,
                            checked_at: current,
                            retry_at: deadline,
                            public_fallback: false,
                        };
                        return Ok(ReleaseCheck {
                            release,
                            notice: save_cache(&cache)
                                .err()
                                .or_else(|| deadline.map(retry_message)),
                            retry_at: deadline,
                        });
                    }
                } else {
                    if let Some(deadline) = rate_limit_deadline(status, response.headers(), current)
                    {
                        let cache = cached.get_or_insert(Cache {
                            release: None,
                            etag: None,
                            checked_at: 0,
                            retry_at: None,
                            public_fallback: false,
                        });
                        cache.retry_at = Some(deadline);
                        if let Err(error) = save_cache(cache) {
                            failure = format!("{error}; ");
                        }
                        failure.push_str(&format!(
                            "GitHub API returned {status}. {}",
                            retry_message(deadline)
                        ));
                    } else {
                        failure = format!("GitHub API returned {status}.");
                    }
                }
            }
            Ok(Err(error)) => failure = format!("Could not check GitHub Releases: {error}"),
            Err(_) => failure = "GitHub Releases did not respond within 30 seconds.".into(),
        }
    }
    match public_release(client).await {
        Ok(release) => {
            let retry_at = cached
                .as_ref()
                .and_then(|cache| cache.retry_at)
                .filter(|time| *time > current);
            let cache = Cache {
                release: Some(release.clone()),
                etag: None,
                checked_at: current,
                retry_at,
                public_fallback: true,
            };
            let save_error = save_cache(&cache)
                .err()
                .map(|error| format!(" {error}"))
                .unwrap_or_default();
            Ok(ReleaseCheck {
                release,
                notice: Some(format!(
                    "{failure} Using verified public release metadata instead.{save_error}"
                )),
                retry_at,
            })
        }
        Err(fallback_error) => {
            let error = format!("{failure} Public release fallback failed: {fallback_error}");
            match cached.and_then(|cache| cache.release.map(|release| (release, cache.retry_at))) {
                Some((release, retry_at)) => Ok(ReleaseCheck {
                    release,
                    notice: Some(format!(
                        "{error} Showing the last saved release; it may be out of date."
                    )),
                    retry_at: retry_at.filter(|time| *time > current),
                }),
                None => Err(error),
            }
        }
    }
}

fn select_release(release: GitHubRelease) -> Result<Release, String> {
    if release.draft || release.prerelease {
        return Err("The latest GitHub release is not stable.".into());
    }
    let expected = format!(
        "swiftify-{}-windows-x86_64.zip",
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
    );
    let mut matches = release
        .assets
        .into_iter()
        .filter(|asset| asset.name.eq_ignore_ascii_case(&expected));
    let asset = matches
        .next()
        .ok_or("The latest release has no Windows x86_64 ZIP asset.")?;
    if matches.next().is_some() {
        return Err(
            "The latest release has multiple Windows x86_64 ZIP assets; unable to choose safely."
                .into(),
        );
    }
    if asset.size == 0 {
        return Err("The Windows release archive is empty.".into());
    }
    Ok(Release {
        version: release.tag_name,
        asset_name: asset.name,
        download_url: asset.browser_download_url,
        size: asset.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_asset_matching_the_stable_tag() {
        let release = GitHubRelease {
            tag_name: "v0.25.1".into(),
            draft: false,
            prerelease: false,
            assets: vec![
                GitHubAsset { name: "swiftify-0.25.1-linux-x86_64.zip".into(), browser_download_url: String::new(), size: 100 },
                GitHubAsset { name: "swiftify-0.25.0-windows-x86_64.zip".into(), browser_download_url: String::new(), size: 100 },
                GitHubAsset { name: "swiftify-0.25.1-windows-x86_64.zip".into(), browser_download_url: "https://github.com/swiftify-org/swiftify/releases/download/v0.25.1/swiftify-0.25.1-windows-x86_64.zip".into(), size: 100 },
            ],
        };
        let result = select_release(release).unwrap();
        assert_eq!(result.version, "v0.25.1");
        assert_eq!(result.asset_name, "swiftify-0.25.1-windows-x86_64.zip");
    }

    #[test]
    fn saves_release_etag_and_retry_deadline_between_runs() {
        let root = std::env::temp_dir().join(format!("swiftify-cache-{}", uuid::Uuid::new_v4()));
        let path = root.join("release-cache.json");
        let cache = Cache {
            release: Some(Release {
                version: "v0.25.1".into(),
                asset_name: "archive.zip".into(),
                download_url:
                    "https://github.com/swiftify-org/swiftify/releases/download/v0.25.1/archive.zip"
                        .into(),
                size: 42,
            }),
            etag: Some("\"etag\"".into()),
            checked_at: 1000,
            retry_at: Some(1800),
            public_fallback: false,
        };
        save_cache_at(&path, &cache).unwrap();
        save_cache_at(&path, &cache).unwrap();
        let saved = read_cache_at(&path).unwrap().unwrap();
        assert_eq!(saved.etag.as_deref(), Some("\"etag\""));
        assert_eq!(saved.retry_at, Some(1800));
        assert_eq!(saved.release.unwrap().size, 42);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires public GitHub access"]
    async fn public_release_provides_a_real_windows_archive() {
        let release = public_release(&client().unwrap()).await.unwrap();
        assert!(release.asset_name.ends_with("-windows-x86_64.zip"));
        assert!(release.size > 0);
        assert!(release
            .download_url
            .starts_with("https://github.com/swiftify-org/swiftify/releases/download/"));
    }

    #[test]
    fn reads_rate_limit_headers_and_uses_a_safe_default() {
        let mut headers = header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", "1800".parse().unwrap());
        assert_eq!(
            rate_limit_deadline(StatusCode::FORBIDDEN, &headers, 1000),
            Some(1800)
        );
        assert_eq!(exhausted_deadline(&headers, 1000), Some(1800));
        headers.insert("x-ratelimit-remaining", "20".parse().unwrap());
        assert_eq!(exhausted_deadline(&headers, 1000), None);
        headers.insert(header::RETRY_AFTER, "1200".parse().unwrap());
        assert_eq!(
            rate_limit_deadline(StatusCode::TOO_MANY_REQUESTS, &headers, 1000),
            Some(2200)
        );
        assert_eq!(
            rate_limit_deadline(StatusCode::FORBIDDEN, &header::HeaderMap::new(), 1000),
            Some(1900)
        );
        assert_eq!(
            rate_limit_deadline(StatusCode::INTERNAL_SERVER_ERROR, &headers, 1000),
            None
        );
    }
}
