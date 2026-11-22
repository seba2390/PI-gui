//! Checking GitHub for a newer release. Nothing is downloaded or installed here: when there
//! is one, the app asks and opens the download in the browser.

use serde::Deserialize;

/// This build's version.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The version updates are compared with: [`CURRENT_VERSION`], or in debug builds
/// `PISTACHIO_PRETEND_VERSION` (to see the update prompt during development).
pub fn compared_version() -> String {
    #[cfg(debug_assertions)]
    if let Ok(version) = std::env::var("PISTACHIO_PRETEND_VERSION") {
        return version;
    }
    CURRENT_VERSION.to_owned()
}

const LATEST_URL: &str = "https://api.github.com/repos/seba2390/pistachio/releases/latest";

/// The answer is a few kilobytes; anything far bigger is not GitHub's release JSON.
const MAX_RESPONSE: usize = 1024 * 1024;

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the leading `v`, e.g. `0.3.0`.
    pub version: String,
    /// The release page.
    pub page_url: String,
    /// The download for this platform, if the release has one.
    pub download_url: Option<String>,
    /// The release notes (Markdown).
    pub notes: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// The platform's part of release file names (Linux builds are per architecture).
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else if cfg!(target_arch = "aarch64") {
        "linux-aarch64"
    } else {
        "linux-x86_64"
    }
}

/// Parses GitHub's `releases/latest` answer; drafts and pre-releases are not offered.
pub fn parse_latest(json: &str, platform: &str) -> Result<Option<Release>, String> {
    let release: GithubRelease =
        serde_json::from_str(json).map_err(|error| format!("unexpected answer: {error}"))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let download_url = release
        .assets
        .iter()
        .find(|asset| asset.name.to_ascii_lowercase().contains(platform))
        .map(|asset| asset.browser_download_url.clone());
    Ok(Some(Release {
        version: release.tag_name.trim_start_matches('v').to_owned(),
        page_url: release.html_url,
        download_url,
        notes: release.body.unwrap_or_default(),
    }))
}

/// Whether `candidate` is a newer version than `current` (`major.minor.patch`; anything
/// unparsable is never newer).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    );
    parts.next().is_none().then_some(version)
}

/// Asks GitHub for the latest release (blocking; run it in the background). One request,
/// with no data about the user.
pub fn fetch_latest() -> Result<Option<Release>, String> {
    let agent = deepsearch::http::backend_agent(10, &format!("pistachio/{CURRENT_VERSION}"));
    let mut response = agent
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("could not reach GitHub: {error}"))?;
    let status = response.status().as_u16();
    if status == 404 {
        return Ok(None); // no release published yet
    }
    if status != 200 {
        return Err(format!("GitHub answered with status {status}"));
    }
    let (bytes, _) = deepsearch::http::read_body(response.body_mut(), MAX_RESPONSE)
        .map_err(|error| format!("could not read GitHub's answer: {error}"))?;
    parse_latest(&String::from_utf8_lossy(&bytes), platform())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST: &str = r#"{
        "tag_name": "v0.3.0",
        "html_url": "https://github.com/seba2390/pistachio/releases/tag/v0.3.0",
        "body": "Hello\n\n- one\n- two",
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": "Pistachio-v0.3.0-macos-universal.zip", "browser_download_url": "https://x/mac.zip"},
            {"name": "pistachio-v0.3.0-linux-aarch64.tar.gz", "browser_download_url": "https://x/linux-arm.tgz"},
            {"name": "pistachio-v0.3.0-linux-x86_64.tar.gz", "browser_download_url": "https://x/linux.tgz"},
            {"name": "pistachio-v0.3.0-windows-x86_64.zip", "browser_download_url": "https://x/win.zip"}
        ]
    }"#;

    #[test]
    fn picks_this_platforms_download() {
        let release = parse_latest(LATEST, "macos").unwrap().unwrap();
        assert_eq!(release.version, "0.3.0");
        assert_eq!(release.download_url.as_deref(), Some("https://x/mac.zip"));
        let windows = parse_latest(LATEST, "windows").unwrap().unwrap();
        assert_eq!(windows.download_url.as_deref(), Some("https://x/win.zip"));
        let linux = parse_latest(LATEST, "linux-x86_64").unwrap().unwrap();
        assert_eq!(linux.download_url.as_deref(), Some("https://x/linux.tgz"));
        let arm = parse_latest(LATEST, "linux-aarch64").unwrap().unwrap();
        assert_eq!(arm.download_url.as_deref(), Some("https://x/linux-arm.tgz"));
        let other = parse_latest(LATEST, "freebsd").unwrap().unwrap();
        assert_eq!(other.download_url, None);
    }

    #[test]
    fn prereleases_and_bad_answers_are_not_offered() {
        let pre = LATEST.replace("\"prerelease\": false", "\"prerelease\": true");
        assert_eq!(parse_latest(&pre, "macos").unwrap(), None);
        assert!(parse_latest("{\"message\": \"rate limited\"}", "macos").is_err());
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.3.0-beta", "0.3.0"));
        assert!(!is_newer("nightly", "0.2.0"));
    }
}
