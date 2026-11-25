//! The helper's side: `<app> --documents-tool pages`. The first line of its input is
//! `{"path"}`, answered with the sizes of the pages; each other one is a [`PageRequest`], answered
//! with a line about the picture and then its pixels. It ends with its input.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use serde::{Deserialize, Serialize};

use super::render::Drawing;
use super::{Link, PageRequest, Picture, Size};
use crate::cli::Request;

pub(crate) const COMMAND: &str = "pages";
/// How long opening a file or drawing a page may take.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Deeply nested PDFs need more stack than a thread usually has.
const STACK: usize = 64 * 1024 * 1024;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Answer {
    Opened {
        pages: Vec<Size>,
    },
    Drawn {
        width: u32,
        height: u32,
        links: Vec<Link>,
    },
    Failed {
        error: String,
    },
}

type Drawn = Result<(Answer, Vec<u8>), String>;

pub(crate) fn run() {
    let (requests, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if requests.send(line).is_err() {
                break;
            }
        }
        // Also in the middle of a page: nobody looks at it any more.
        std::process::exit(0);
    });
    let mut out = io::stdout().lock();
    if let Err(error) = serve(&lines, &mut out) {
        eprintln!("{error}");
    }
}

/// Answers `lines`. Returns when there are no more, or when one takes too long.
pub(super) fn serve(lines: &mpsc::Receiver<String>, out: &mut impl Write) -> io::Result<()> {
    let Ok(first) = lines.recv() else {
        return Ok(());
    };
    let (ask, asked) = mpsc::channel::<PageRequest>();
    let (answer, answers) = mpsc::channel::<Drawn>();
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || draw_all(&first, &asked, &answer))?;
    loop {
        let Ok(answered) = answers.recv_timeout(TIMEOUT) else {
            return write(out, &failed("it takes too long to draw"), &[]);
        };
        match answered {
            Ok((header, pixels)) => write(out, &header, &pixels)?,
            Err(error) => write(out, &failed(&error), &[])?,
        }
        // What is no request is not answered.
        let request = loop {
            let Ok(line) = lines.recv() else {
                return Ok(());
            };
            if let Ok(request) = serde_json::from_str(&line) {
                break request;
            }
        };
        if ask.send(request).is_err() {
            return Ok(());
        }
    }
}

fn draw_all(first: &str, asked: &mpsc::Receiver<PageRequest>, answer: &mpsc::Sender<Drawn>) {
    let pdf = match open(first) {
        Ok(pdf) => pdf,
        Err(error) => {
            let _ = answer.send(Err(error));
            return;
        }
    };
    let drawing = Drawing::new(pdf.pages());
    let pages = drawing.sizes();
    if pages.is_empty() {
        let _ = answer.send(Err("it has no pages".into()));
        return;
    }
    let mut next = answer.send(Ok((Answer::Opened { pages }, Vec::new())));
    while next.is_ok() {
        let Ok(request) = asked.recv() else { return };
        next = answer.send(drawing.draw(&request).map(
            |Picture {
                 width,
                 height,
                 links,
                 pixels,
             }| {
                let header = Answer::Drawn {
                    width,
                    height,
                    links,
                };
                (header, pixels)
            },
        ));
    }
}

fn open(request: &str) -> Result<Pdf, String> {
    let request: Request =
        serde_json::from_str(request).map_err(|error| format!("invalid input: {error}"))?;
    let bytes = read(&request.path)?;
    Pdf::new(bytes).map_err(|error| {
        match error {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                "it is protected with a password"
            }
            LoadPdfError::Decryption(_) => "it is encrypted in a way that is not supported",
            LoadPdfError::Invalid => "it is not a PDF file, or it is damaged",
        }
        .to_owned()
    })
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("it is not a file".into());
    }
    if meta.len() > MAX_BYTES {
        return Err("it is larger than 1 GB".into());
    }
    std::fs::read(path).map_err(|error| error.to_string())
}

fn failed(error: &str) -> Answer {
    Answer::Failed {
        error: error.to_owned(),
    }
}

fn write(out: &mut impl Write, header: &Answer, pixels: &[u8]) -> io::Result<()> {
    serde_json::to_writer(&mut *out, header)?;
    out.write_all(b"\n")?;
    out.write_all(pixels)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing;
    use super::*;

    fn serve_lines(lines: &[String]) -> Vec<u8> {
        let (sender, receiver) = mpsc::channel();
        for line in lines {
            sender.send(line.clone()).unwrap();
        }
        drop(sender);
        let mut out = Vec::new();
        serve(&receiver, &mut out).unwrap();
        out
    }

    fn opening(contents: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        std::fs::write(&path, contents).unwrap();
        let line = serde_json::to_string(&Request { path }).unwrap();
        (dir, line)
    }

    #[test]
    fn answers_in_order() {
        let (_dir, first) = opening(&testing::pdf(1, ""));
        let request = |page| {
            let request = PageRequest {
                page,
                scale: 1.,
                dark: false,
            };
            serde_json::to_string(&request).unwrap()
        };
        let out = serve_lines(&[first, request(0), "nonsense".into(), request(5)]);
        let mut rest = out.as_slice();
        let mut line = String::new();
        rest.read_line(&mut line).unwrap();
        assert_eq!(
            line,
            "{\"opened\":{\"pages\":[{\"width\":100.0,\"height\":200.0}]}}\n"
        );
        line.clear();
        rest.read_line(&mut line).unwrap();
        assert_eq!(
            line,
            "{\"drawn\":{\"width\":100,\"height\":200,\"links\":[]}}\n"
        );
        let rest = std::str::from_utf8(&rest[100 * 200 * 4..]).unwrap();
        assert_eq!(rest, "{\"failed\":{\"error\":\"there is no such page\"}}\n");
    }

    #[test]
    fn files_that_cannot_be_opened() {
        let error = |contents: &[u8]| {
            let (_dir, first) = opening(contents);
            String::from_utf8(serve_lines(&[first])).unwrap()
        };
        let damaged = error(b"not a PDF");
        assert!(damaged.contains("damaged"), "{damaged}");
        let missing = serve_lines(&["{\"path\":\"/nonexistent/a.pdf\"}".into()]);
        assert!(missing.starts_with(b"{\"failed\""));
        assert_eq!(serve_lines(&[]), [] as [u8; 0]);
    }
}
