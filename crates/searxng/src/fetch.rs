//! Downloading and unpacking a pinned archive.

use std::io::Read as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use sha2::{Digest as _, Sha256};

use crate::InstallError;

/// Unpacked archives larger than this are refused.
const MAX_UNPACKED: u64 = 256 * 1024 * 1024;
/// Compressed archives may be this much larger than expected before the download stops.
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// What the checksum of an archive is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumOf {
    Compressed,
    Tar,
}

/// Downloads `url` (a `.tar.gz`) and returns the tar it holds, if it has the checksum. Reports
/// the bytes received so far.
pub fn download_tar(
    url: &str,
    sha256: &str,
    of: ChecksumOf,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, InstallError> {
    let failed = |error: &dyn std::fmt::Display| {
        InstallError::Failed(format!("Downloading {url} failed: {error}"))
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent("pistachio")
        .build()
        .new_agent();
    let mut response = agent.get(url).call().map_err(|error| failed(&error))?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .reader();
    let mut archive = Vec::new();
    let mut buffer = vec![0u8; 128 * 1024];
    let mut reported = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let n = reader.read(&mut buffer).map_err(|error| failed(&error))?;
        if n == 0 {
            break;
        }
        archive.extend_from_slice(&buffer[..n]);
        if reported.elapsed() >= Duration::from_millis(100) {
            progress(archive.len() as u64);
            reported = Instant::now();
        }
    }
    progress(archive.len() as u64);

    let mismatch = || {
        InstallError::Failed(format!(
            "{url} did not match its checksum, so it was not used; try again"
        ))
    };
    let matches = |bytes: &[u8]| hex(&Sha256::digest(bytes)) == sha256;
    if of == ChecksumOf::Compressed && !matches(&archive) {
        return Err(mismatch());
    }
    let mut tar = Vec::new();
    GzDecoder::new(archive.as_slice())
        .take(MAX_UNPACKED)
        .read_to_end(&mut tar)
        .map_err(|_| mismatch())?;
    if of == ChecksumOf::Tar && !matches(&tar) {
        return Err(mismatch());
    }
    Ok(tar)
}

/// Unpacks the files of `tar` that `wanted` picks by their path without the archive's single top
/// folder.
pub fn unpack(tar: &[u8], into: &Path, wanted: impl Fn(&Path) -> bool) -> std::io::Result<()> {
    for entry in tar::Archive::new(tar).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Ok(path) = path.strip_prefix(
            path.components()
                .next()
                .map_or("".as_ref(), |c| AsRef::<Path>::as_ref(c.as_os_str())),
        ) else {
            continue;
        };
        let plain = path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
        if path.as_os_str().is_empty() || !plain || !wanted(path) {
            continue;
        }
        let target = into.join(path);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?;
        }
        // No links: neither archive needs one, and they could lead out of `into`.
    }
    Ok(())
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use pretty_assertions::assert_eq;

    use super::*;

    fn tar(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            // Set directly: `set_path` refuses paths that lead out.
            header.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_cksum();
            builder.append(&header, text.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// Serves `body` once and returns its URL.
    fn serve(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/archive.tar.gz", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        });
        url
    }

    fn download(body: Vec<u8>, sha256: &str, of: ChecksumOf) -> Result<Vec<u8>, InstallError> {
        let cancel = AtomicBool::new(false);
        download_tar(&serve(body), sha256, of, &cancel, &mut |_| {})
    }

    #[test]
    fn archives_are_checked() {
        let tar = tar(&[("top/a.txt", "a")]);
        let archive = gzip(&tar);
        let sha256 = |bytes: &[u8]| hex(&Sha256::digest(bytes));
        assert_eq!(
            download(archive.clone(), &sha256(&archive), ChecksumOf::Compressed).unwrap(),
            tar
        );
        assert_eq!(
            download(archive.clone(), &sha256(&tar), ChecksumOf::Tar).unwrap(),
            tar
        );
        for of in [ChecksumOf::Compressed, ChecksumOf::Tar] {
            let error = download(archive.clone(), &sha256(b"other"), of).unwrap_err();
            assert!(error.to_string().contains("checksum"), "{error}");
        }
    }

    #[test]
    fn a_cancelled_download_stops() {
        let url = serve(gzip(&tar(&[("top/a.txt", "a")])));
        let cancel = AtomicBool::new(true);
        let result = download_tar(&url, "", ChecksumOf::Tar, &cancel, &mut |_| {});
        assert!(matches!(result, Err(InstallError::Cancelled)));
    }

    #[test]
    fn unpacks_what_is_wanted_and_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let into = dir.path().join("into");
        let tar = tar(&[
            ("top/searx/a.py", "a"),
            ("top/docs/b.rst", "b"),
            ("top/searx/../../../out.txt", "no"),
            ("/searx/abs.py", "no"),
        ]);
        unpack(&tar, &into, |path| path.starts_with("searx")).unwrap();
        assert_eq!(
            std::fs::read_to_string(into.join("searx/a.py")).unwrap(),
            "a"
        );
        assert!(!into.join("docs").exists());
        assert!(!dir.path().join("out.txt").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
