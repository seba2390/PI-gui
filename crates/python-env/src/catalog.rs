//! What is downloaded: pinned versions and checksums. `script/update-searxng.sh` moves uv's.

pub const UV_VERSION: &str = "0.13.9";
/// uv knows the checksums of the Pythons it downloads.
pub const PYTHON_VERSION: &str = "3.13.18";

/// What differs between computers.
#[derive(Debug, PartialEq, Eq)]
pub struct Platform {
    /// As in the name of uv's archive.
    pub target: &'static str,
    pub uv_sha256: &'static str,
    /// Download sizes in bytes; the last is only shown.
    pub uv_size: u64,
    pub python_size: u64,
}

pub static PLATFORMS: &[Platform] = &[
    Platform {
        target: "aarch64-apple-darwin",
        uv_sha256: "5aeca67f2283f75235de40d0bd9c0a5bbc5fce23b6d48b3bf9fa1b2db08382ca",
        uv_size: 14_554_974,
        python_size: 25_479_437,
    },
    Platform {
        target: "x86_64-apple-darwin",
        uv_sha256: "617a3211261b0fc739562f0071aa34e9739797c510208ede8e2977c30903d041",
        uv_size: 17_362_924,
        python_size: 25_250_783,
    },
    Platform {
        target: "x86_64-unknown-linux-gnu",
        uv_sha256: "a7fbaff2b9a271d85c1bdb540941090fd1ce011797b4abeb48e4dcd39c2471d5",
        uv_size: 17_068_176,
        python_size: 35_596_130,
    },
    Platform {
        target: "aarch64-unknown-linux-gnu",
        uv_sha256: "0797542018e541a9288855df555c8077eed06f7a47d206362030480e8ecc9f3d",
        uv_size: 16_120_245,
        python_size: 29_683_231,
    },
];

/// This computer's; `None` where there is none (Windows, and Linux without glibc).
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_is_pinned() {
        for platform in PLATFORMS {
            assert_eq!(platform.uv_sha256.len(), 64);
            assert!(platform.uv_sha256.chars().all(|c| c.is_ascii_hexdigit()));
            let name = format!("uv-{}.tar.gz", platform.target);
            assert!(platform.uv_url().ends_with(&name));
        }
    }

    #[test]
    fn macos_and_linux_have_a_platform() {
        assert_eq!(
            platform().is_some(),
            cfg!(any(target_os = "macos", target_os = "linux"))
        );
    }
}
