//! What is downloaded: pinned versions and checksums. `script/update-searxng.sh` moves the pins.

pub use python_env::catalog::{PYTHON_VERSION, UV_VERSION};

/// SearXNG has no releases, so a commit.
pub const SEARXNG_COMMIT: &str = "a7163d091037cae586c444b05f22ecca162bb492";
/// Of the uncompressed archive: GitHub makes it on request and may compress it differently.
pub const SEARXNG_TAR_SHA256: &str =
    "da9a22f48a2479a85786b525fa991bc24d07c203883b5d39267982a3cec7d67e";
pub const SEARXNG_SIZE: u64 = 6_743_148;

/// The packages SearXNG needs, each with the checksums of its files.
pub const REQUIREMENTS: &str = include_str!("requirements.txt");

/// What differs between computers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub python: &'static python_env::catalog::Platform,
    /// Download size in bytes; only shown.
    pub packages_size: u64,
}

/// This computer's; `None` where SearXNG does not run (it does not support Windows).
pub fn platform() -> Option<Platform> {
    let python = python_env::catalog::platform()?;
    let packages_size = match python.target {
        "aarch64-apple-darwin" => 28_800_000,
        "x86_64-apple-darwin" => 25_300_000,
        "x86_64-unknown-linux-gnu" => 37_300_000,
        "aarch64-unknown-linux-gnu" => 36_400_000,
        _ => return None,
    };
    Some(Platform {
        python,
        packages_size,
    })
}

impl Platform {
    /// The whole download in bytes.
    pub fn download_size(&self) -> u64 {
        self.python.uv_size + SEARXNG_SIZE + self.python.python_size + self.packages_size
    }
}

pub fn searxng_url() -> String {
    format!("https://codeload.github.com/searxng/searxng/tar.gz/{SEARXNG_COMMIT}")
}

/// How many packages [`REQUIREMENTS`] installs.
pub fn package_count() -> usize {
    REQUIREMENTS
        .lines()
        .filter(|line| line.contains("==") && !line.contains("win32"))
        .count()
}

/// Says what an installation was made of: another one than the app's is out of date.
pub fn revision() -> String {
    let packages = python_env::fetch::sha256(REQUIREMENTS.as_bytes());
    format!("{SEARXNG_COMMIT} python-{PYTHON_VERSION} packages-{packages}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_pinned() {
        let sha256 = |text: &str| text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit());
        assert!(sha256(SEARXNG_TAR_SHA256));
        assert_eq!(SEARXNG_COMMIT.len(), 40);

        // Every requirement names one version and at least one checksum.
        let mut requirements = Vec::new();
        for line in REQUIREMENTS.lines() {
            if line.starts_with(' ') {
                assert!(line.trim().starts_with("--hash=sha256:"), "{line}");
                *requirements.last_mut().unwrap() += 1;
            } else {
                assert!(line.contains("==") && line.ends_with('\\'), "{line}");
                requirements.push(0);
            }
        }
        assert!(requirements.len() > 20 && requirements.iter().all(|hashes| *hashes > 0));
        assert!((20..=requirements.len()).contains(&package_count()));
    }

    #[test]
    fn only_systems_searxng_runs_on_have_a_platform() {
        assert_eq!(
            platform().is_some(),
            cfg!(any(target_os = "macos", target_os = "linux"))
        );
        if let Some(platform) = platform() {
            assert!((50_000_000..150_000_000).contains(&platform.download_size()));
        }
    }
}
