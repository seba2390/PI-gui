//! The parsers, and what is downloaded for each: locked packages and models, with checksums.
//! `script/update-parsers.sh` moves the pins (the files in `locks/`).

use python_env::catalog::{PYTHON_VERSION, Platform};
use python_env::fetch::{Pinned, sha256};
use serde::{Deserialize, Serialize};

/// The oldest macOS the packages run on.
pub const OLDEST_MACOS: u32 = 14;
/// A Python on disk, about.
const PYTHON_DISK: u64 = 90_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parser {
    /// What comes with the app.
    #[default]
    Builtin,
    Pymupdf4llm,
    Docling,
    Mineru,
}

impl Parser {
    pub const ALL: [Self; 4] = [
        Self::Builtin,
        Self::Pymupdf4llm,
        Self::Docling,
        Self::Mineru,
    ];

    /// As it is saved, and the name of its folder.
    pub fn id(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Pymupdf4llm => "pymupdf4llm",
            Self::Docling => "docling",
            Self::Mineru => "mineru",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|parser| parser.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Builtin => "Built-in",
            Self::Pymupdf4llm => "PyMuPDF4LLM",
            Self::Docling => "Docling",
            Self::Mineru => "MinerU",
        }
    }

    /// As in a sentence.
    pub fn name(self) -> &'static str {
        match self {
            Self::Builtin => "the built-in reader",
            other => other.label(),
        }
    }

    /// What it is good at.
    pub fn strengths(self) -> &'static str {
        match self {
            Self::Builtin => {
                "The text of PDFs, page by page, and of Word, PowerPoint, Excel, OpenDocument and EPUB \
                 files. Fast, and nothing to download."
            }
            Self::Pymupdf4llm => {
                "Fast. Headings, columns and tables of PDFs made on a computer. No scans or \
                 formulas."
            }
            Self::Docling => {
                "Complex layouts, reading order, tables and scans. Also for Word, PowerPoint and \
                 Excel files in chats."
            }
            Self::Mineru => {
                "Academic papers: formulas as LaTeX, dense columns, long tables and scans. The \
                 slowest."
            }
        }
    }

    pub fn license(self) -> &'static str {
        match self {
            Self::Builtin => "MIT",
            Self::Pymupdf4llm => "AGPL-3.0",
            Self::Docling => "MIT",
            Self::Mineru => "MinerU Open Source License",
        }
    }

    /// What there is to know about the license besides its name.
    pub fn license_note(self) -> &'static str {
        match self {
            Self::Builtin => "",
            Self::Pymupdf4llm => {
                "AGPL-3.0, or a commercial license from Artifex. Pistachio runs it as a separate \
                 program and does not include it."
            }
            Self::Docling => "MIT. Its models are Apache-2.0 and CDLA-Permissive-2.0.",
            Self::Mineru => {
                "Apache-2.0 with added terms: a company with more than 100 million monthly users \
                 or 20 million US dollars of monthly revenue needs a commercial license, and online \
                 services built on it must say so."
            }
        }
    }

    /// Whether it reads scans (pages that are pictures).
    pub fn reads_scans(self) -> bool {
        self.has_models()
    }

    /// Whether it loads models, so that only one of it should run.
    pub(crate) fn has_models(self) -> bool {
        matches!(self, Self::Docling | Self::Mineru)
    }

    /// The packages of it for the computers called `target`; `None` where it does not run.
    fn lock(self, target: &str) -> Option<Lock> {
        let text = match (self, target) {
            (Self::Pymupdf4llm, "aarch64-apple-darwin") => {
                include_str!("locks/pymupdf4llm-aarch64-apple-darwin.txt")
            }
            (Self::Pymupdf4llm, "x86_64-apple-darwin") => {
                include_str!("locks/pymupdf4llm-x86_64-apple-darwin.txt")
            }
            (Self::Pymupdf4llm, "x86_64-unknown-linux-gnu") => {
                include_str!("locks/pymupdf4llm-x86_64-unknown-linux-gnu.txt")
            }
            (Self::Pymupdf4llm, "aarch64-unknown-linux-gnu") => {
                include_str!("locks/pymupdf4llm-aarch64-unknown-linux-gnu.txt")
            }
            (Self::Docling, "aarch64-apple-darwin") => {
                include_str!("locks/docling-aarch64-apple-darwin.txt")
            }
            (Self::Docling, "x86_64-unknown-linux-gnu") => {
                include_str!("locks/docling-x86_64-unknown-linux-gnu.txt")
            }
            (Self::Docling, "aarch64-unknown-linux-gnu") => {
                include_str!("locks/docling-aarch64-unknown-linux-gnu.txt")
            }
            (Self::Mineru, "aarch64-apple-darwin") => {
                include_str!("locks/mineru-aarch64-apple-darwin.txt")
            }
            (Self::Mineru, "x86_64-unknown-linux-gnu") => {
                include_str!("locks/mineru-x86_64-unknown-linux-gnu.txt")
            }
            (Self::Mineru, "aarch64-unknown-linux-gnu") => {
                include_str!("locks/mineru-aarch64-unknown-linux-gnu.txt")
            }
            _ => return None,
        };
        Some(Lock { text })
    }

    /// The models it needs.
    pub fn models(self) -> impl Iterator<Item = Model> {
        let text = match self {
            Self::Builtin | Self::Pymupdf4llm => "",
            Self::Docling => include_str!("locks/docling-models.txt"),
            Self::Mineru => include_str!("locks/mineru-models.txt"),
        };
        text.lines().filter_map(|line| {
            let mut parts = line.split(' ');
            Some(Model {
                sha256: parts.next()?,
                size: parts.next()?.parse().ok()?,
                url: parts.next()?,
                path: parts.next()?,
            })
        })
    }

    /// What is downloaded for it on this computer, or why it cannot be.
    pub fn download(self) -> Result<Download, &'static str> {
        let Some(platform) = python_env::catalog::platform() else {
            return Err("Not available on this system");
        };
        if macos_version().is_some_and(|version| version < OLDEST_MACOS) {
            return Err("Needs macOS 14 or newer");
        }
        let Some(lock) = self.lock(platform.target) else {
            return Err(if platform.target == "x86_64-apple-darwin" {
                "Not available on Macs with an Intel processor"
            } else {
                "Not available on this system"
            });
        };
        Ok(Download {
            parser: self,
            platform,
            lock,
        })
    }
}

/// A file of a parser's models, and where it goes in the folder for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub sha256: &'static str,
    pub size: u64,
    pub url: &'static str,
    pub path: &'static str,
}

impl Model {
    pub fn pinned(&self) -> Pinned<'static> {
        Pinned {
            url: self.url,
            size: self.size,
            sha256: self.sha256,
        }
    }
}

/// Locked packages: comments with their version and size, then one package per line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lock {
    text: &'static str,
}

impl Lock {
    fn comment(self, key: &str) -> &'static str {
        self.text
            .lines()
            .filter_map(|line| line.strip_prefix("# "))
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
            .unwrap_or_default()
    }
}

/// What is downloaded for a parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Download {
    parser: Parser,
    pub platform: &'static Platform,
    lock: Lock,
}

impl Download {
    pub fn parser(&self) -> Parser {
        self.parser
    }

    pub fn version(&self) -> &'static str {
        self.lock.comment("version")
    }

    /// What uv installs, for `--require-hashes`.
    pub fn requirements(&self) -> &'static str {
        self.lock.text
    }

    pub fn package_count(&self) -> usize {
        let lines = self.lock.text.lines();
        lines.filter(|line| line.contains("==")).count()
    }

    pub fn packages_size(&self) -> u64 {
        self.lock.comment("download").parse().unwrap_or_default()
    }

    pub fn models_size(&self) -> u64 {
        self.parser.models().map(|model| model.size).sum()
    }

    /// The whole download in bytes.
    pub fn size(&self) -> u64 {
        self.platform.uv_size
            + self.platform.python_size
            + self.packages_size()
            + self.models_size()
    }

    /// What it takes on disk, about. Packages are compressed when they are downloaded; by how
    /// much was measured on a Mac. On Linux, Docling has about 3 GB of NVIDIA's libraries more,
    /// which compress less.
    pub fn disk_size(&self) -> u64 {
        const NVIDIA: u64 = 500_000_000;
        let size = self.packages_size();
        let packages = match self.parser {
            Parser::Pymupdf4llm => size * 22 / 10,
            Parser::Docling => size.min(NVIDIA) * 36 / 10 + size.saturating_sub(NVIDIA) * 2,
            Parser::Builtin | Parser::Mineru => size * 32 / 10,
        };
        PYTHON_DISK + packages + self.models_size()
    }

    /// Says what an installation was made of: another one than the app's is out of date.
    pub fn revision(&self) -> String {
        let models: String = self.parser.models().map(|model| model.sha256).collect();
        format!(
            "{} {} python-{PYTHON_VERSION} packages-{} models-{}",
            self.parser.id(),
            self.version(),
            sha256(self.lock.text.as_bytes()),
            sha256(models.as_bytes()),
        )
    }
}

/// The major version of macOS, where this is one.
fn macos_version() -> Option<u32> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let file = "/System/Library/CoreServices/SystemVersion.plist";
    product_version(&std::fs::read_to_string(file).ok()?)
}

fn product_version(plist: &str) -> Option<u32> {
    let after = plist.split("<key>ProductVersion</key>").nth(1)?;
    let version = after.split("<string>").nth(1)?.split("</string>").next()?;
    version.split('.').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const DOWNLOADS: [Parser; 3] = [Parser::Pymupdf4llm, Parser::Docling, Parser::Mineru];

    fn locks() -> impl Iterator<Item = (Parser, &'static Platform, Lock)> {
        DOWNLOADS.into_iter().flat_map(|parser| {
            python_env::catalog::PLATFORMS
                .iter()
                .filter_map(move |platform| Some((parser, platform, parser.lock(platform.target)?)))
        })
    }

    #[test]
    fn everything_is_pinned() {
        let sha256 = |text: &str| text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit());
        for (parser, platform, lock) in locks() {
            let name = format!("{} {}", parser.id(), platform.target);
            assert!(!lock.comment("version").is_empty(), "{name}");
            assert!(
                lock.comment("download").parse::<u64>().is_ok_and(|n| n > 0),
                "{name}"
            );
            let mut packages = 0;
            for line in lock.text.lines().filter(|line| !line.starts_with('#')) {
                let (package, hashes) = line.split_once(' ').unwrap();
                assert!(package.contains("=="), "{name}: {line}");
                for hash in hashes.split(' ') {
                    let hash = hash.strip_prefix("--hash=sha256:").unwrap();
                    assert!(sha256(hash), "{name}: {line}");
                }
                packages += 1;
            }
            assert!(packages > 10, "{name}");
        }
        for parser in DOWNLOADS {
            for model in parser.models() {
                assert!(sha256(model.sha256) && model.size > 0, "{model:?}");
                // A commit, not a branch.
                let commit = model.url.split("/resolve/").nth(1).unwrap();
                assert_eq!(commit.split('/').next().unwrap().len(), 40);
                assert!(model.url.starts_with("https://huggingface.co/"));
                assert!(!model.path.contains(".."));
            }
        }
        assert_eq!(Parser::Docling.models().count(), 5);
        assert_eq!(
            Parser::Mineru.models().count(),
            include_str!("locks/mineru-models.txt").lines().count()
        );
    }

    #[test]
    fn systems_without_wheels_are_left_out() {
        let on = |parser: Parser| -> Vec<&str> {
            locks()
                .filter(|(other, _, _)| *other == parser)
                .map(|(_, platform, _)| platform.target)
                .collect()
        };
        assert_eq!(on(Parser::Pymupdf4llm).len(), 4);
        for parser in [Parser::Docling, Parser::Mineru] {
            assert_eq!(on(parser).len(), 3);
            assert!(!on(parser).contains(&"x86_64-apple-darwin"));
        }
        assert!(Parser::Builtin.download().is_err());
    }

    #[test]
    fn sizes_are_what_the_locks_say() {
        for parser in DOWNLOADS {
            let Ok(download) = parser.download() else {
                continue;
            };
            assert!(download.size() > download.packages_size() + download.models_size());
            assert!(download.disk_size() > download.size());
            assert!(download.revision().starts_with(parser.id()));
            assert!(download.package_count() > 10);
        }
    }

    #[test]
    fn ids_are_stable() {
        let ids = Parser::ALL.map(Parser::id);
        assert_eq!(ids, ["builtin", "pymupdf4llm", "docling", "mineru"]);
        for parser in Parser::ALL {
            assert_eq!(Parser::from_id(parser.id()), Some(parser));
            let json = serde_json::to_value(parser).unwrap();
            assert_eq!(json, parser.id());
        }
        assert_eq!(Parser::from_id("other"), None);
    }

    #[test]
    fn finds_the_macos_version() {
        let plist = "<dict><key>ProductName</key><string>macOS</string>\n\
                     <key>ProductVersion</key>\n\t<string>14.7.1</string></dict>";
        assert_eq!(product_version(plist), Some(14));
        assert_eq!(product_version("<dict></dict>"), None);
        assert_eq!(macos_version().is_some(), cfg!(target_os = "macos"));
    }
}
