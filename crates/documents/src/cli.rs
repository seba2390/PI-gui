//! The helper: `<app> --documents-tool <command>` with JSON (`{"path"}`) on stdin and one JSON
//! reply on stdout ([`Reply`]).
//!
//! Commands: `parse` reads its input to the end; `watched` reads one line, and ends when its input
//! does, which is how the app stops it ([`crate::client`]). `pages` draws the pages of a PDF file
//! ([`crate::viewer`]).
//!
//! To try settings by hand, a path after the command prints the Markdown:
//! `pistachio --documents-tool parse paper.pdf`.

use std::io::{BufRead as _, IsTerminal as _, Read as _};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Parser, config, read, viewer};

pub const FLAG: &str = "--documents-tool";
pub(crate) const WATCHED: &str = "watched";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Request {
    pub path: PathBuf,
}

/// What a document says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub markdown: String,
    /// What read it.
    pub parser: Parser,
    /// Why that is not the chosen parser.
    pub note: Option<String>,
}

/// Runs the helper when the process was started with [`FLAG`], returning its exit code.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(FLAG) {
        return None;
    }
    let command = args.next().unwrap_or_default();
    let by_hand = args.next().map(PathBuf::from);
    // A file that breaks its reader is an answer too, and must not look like a crash of the app.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        println!("\n{}", failure("the built-in reader cannot read it"));
        std::process::exit(1);
    }));
    if command == viewer::serve::COMMAND {
        viewer::serve::run();
        return Some(0);
    }
    let request = match &by_hand {
        Some(path) => Ok(Request { path: path.clone() }),
        None => request(&command),
    };
    let reply = request.and_then(|request| reply(&request));
    let ok = reply.is_ok();
    match (by_hand, reply) {
        (Some(_), Ok(reply)) => println!(
            "{}{}",
            reply.note.map_or_default(|note| format!("({note})\n\n")),
            reply.markdown
        ),
        (Some(_), Err(error)) => eprintln!("{error}"),
        // On a line of its own, whatever a library printed.
        (None, Ok(reply)) => {
            let mut value = serde_json::to_value(&reply).unwrap_or_default();
            value["ok"] = true.into();
            println!("\n{value}");
        }
        (None, Err(error)) => println!("\n{}", failure(&error)),
    }
    Some(i32::from(!ok))
}

fn request(command: &str) -> Result<Request, String> {
    let mut stdin = std::io::stdin();
    let mut text = String::new();
    match command {
        "parse" if !stdin.is_terminal() => {
            let _ = stdin.read_to_string(&mut text);
        }
        WATCHED => {
            let _ = stdin.lock().read_line(&mut text);
            // Also when the app crashes: no document is read for nobody.
            std::thread::spawn(move || {
                let _ = stdin.read_to_end(&mut Vec::new());
                std::process::exit(1);
            });
        }
        _ => return Err(format!("unknown documents command \"{command}\"")),
    }
    serde_json::from_str(&text).map_err(|error| format!("invalid input: {error}"))
}

fn reply(request: &Request) -> Result<Reply, String> {
    let dir = config::default_dir()
        .ok_or("the home folder with the settings of document parsing was not found")?;
    let config = config::load(&dir);
    let document = read(&dir, &config, &request.path, config.timeout())?;
    let (parser, note) = (document.parser, document.note.clone());
    Ok(Reply {
        markdown: document.markdown(),
        parser,
        note,
    })
}

fn failure(error: &str) -> serde_json::Value {
    json!({ "ok": false, "error": error })
}
