//! What is downloaded: pinned versions and checksums. `script/update-searxng.sh` moves the pins.

/// SearXNG has no releases, so a commit.
pub const SEARXNG_COMMIT: &str = "a7163d091037cae586c444b05f22ecca162bb492";
/// Of the uncompressed archive: GitHub makes it on request and may compress it differently.
pub const SEARXNG_TAR_SHA256: &str =
    "da9a22f48a2479a85786b525fa991bc24d07c203883b5d39267982a3cec7d67e";
pub const SEARXNG_SIZE: u64 = 6_743_148;

pub const UV_VERSION: &str = "0.13.9";
/// uv knows the checksums of the Pythons it downloads.
pub const PYTHON_VERSION: &str = "3.13.18";

/// The packages SearXNG needs, each with the checksums of its files.
pub const REQUIREMENTS: &str = include_str!("requirements.txt");

/// What differs between computers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// As in the name of uv's archive.
    pub target: &'static str,
    pub uv_sha256: &'static str,
    /// Download sizes in bytes; the last two are only shown.
    pub uv_size: u64,
    pub python_size: u64,
    pub packages_size: u64,
}

pub const PLATFORMS: &[Platform] = &[
    Platform {
        target: "aarch64-apple-darwin",
        uv_sha256: "5aeca67f2283f75235de40d0bd9c0a5bbc5fce23b6d48b3bf9fa1b2db08382ca",
        uv_size: 14_554_974,
        python_size: 25_479_437,
        packages_size: 28_800_000,
    },
    Platform {
        target: "x86_64-apple-darwin",
        uv_sha256: "617a3211261b0fc739562f0071aa34e9739797c510208ede8e2977c30903d041",
        uv_size: 17_362_924,
        python_size: 25_250_783,
        packages_size: 25_300_000,
    },
    Platform {
        target: "x86_64-unknown-linux-gnu",
        uv_sha256: "a7fbaff2b9a271d85c1bdb540941090fd1ce011797b4abeb48e4dcd39c2471d5",
        uv_size: 17_068_176,
        python_size: 35_596_130,
        packages_size: 37_300_000,
    },
    Platform {
        target: "aarch64-unknown-linux-gnu",
        uv_sha256: "0797542018e541a9288855df555c8077eed06f7a47d206362030480e8ecc9f3d",
        uv_size: 16_120_245,
        python_size: 29_683_231,
        packages_size: 36_400_000,
    },
];

/// This computer's; `None` where SearXNG does not run (it does not support Windows).
pub fn platform() -> Option<&'static Platform> {
    let system = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(all(target_os = "linux", target_env = "gnu")) {
        "unknown-linux-gnu"
    } else {
        return None;
    };
    let target = format!("{}-{system}", std::env::consts::ARCH);
    PLATFORMS.iter().find(|platform| platform.target == target)
}

impl Platform {
    pub fn uv_url(&self) -> String {
        format!(
            "https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-{}.tar.gz",
            self.target
        )
    }

    /// The whole download in bytes.
    pub fn download_size(&self) -> u64 {
        self.uv_size + SEARXNG_SIZE + self.python_size + self.packages_size
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
    use sha2::{Digest as _, Sha256};
    let packages = crate::fetch::hex(&Sha256::digest(REQUIREMENTS));
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
        assert!(PLATFORMS.iter().all(|platform| sha256(platform.uv_sha256)));

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
            assert!(
                platform
                    .uv_url()
                    .ends_with(&format!("uv-{}.tar.gz", platform.target))
            );
            assert!((50_000_000..150_000_000).contains(&platform.download_size()));
        }
    }
}
