//! The helper the pi extension runs: `<app> --deepsearch-tool <command>` with JSON input on
//! stdin and one JSON reply on stdout (`{"ok":true,…}` or `{"ok":false,"error":…}`).
//!
//! Commands: `config` (settings and each mode's instructions), `search` (`{"query",
//! "max_results"?}`), `fetch` (`{"url","focus"?}`), `check` (a report: [`check`]) and `quotes`.
//!
//! For trying settings by hand, words after the command become the input and the reply is
//! printed as the model would see it:
//! `pistachio --deepsearch-tool search rust gpui` or
//! `pistachio --deepsearch-tool fetch https://example.com "what to look for"`.

use std::io::{IsTerminal as _, Read as _};

use serde_json::{Value, json};

use crate::check::{self, CheckInput};
use crate::config::{self, Config};
use crate::effort::Effort;
use crate::tools;

pub const FLAG: &str = "--deepsearch-tool";

/// Runs the helper when the process was started with [`FLAG`], returning its exit code.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(FLAG) {
        return None;
    }
    let command = args.next().unwrap_or_default();
    let words: Vec<String> = args.collect();
    // A file that breaks its reader is an answer too, and must not look like a crash of the app.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        println!(
            "\n{}",
            failure("The page could not be read. Try another source.")
        );
        std::process::exit(1);
    }));
    let by_hand = !words.is_empty();
    let input = if by_hand {
        input_from_words(&command, &words)
    } else {
        read_stdin()
    };
    let reply = run(&command, &input);
    let ok = reply["ok"].as_bool() == Some(true);
    if by_hand {
        if let Some(text) = reply["text"].as_str() {
            println!("{text}");
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&reply).unwrap_or_default()
            );
        }
    } else {
        // On a line of its own, whatever a library printed.
        println!("\n{reply}");
    }
    Some(i32::from(!ok))
}

fn input_from_words(command: &str, words: &[String]) -> Value {
    match command {
        "fetch" => json!({
            "url": words[0],
            "focus": (words.len() > 1).then(|| words[1..].join(" ")),
        }),
        _ => json!({ "query": words.join(" ") }),
    }
}

fn read_stdin() -> Value {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Value::Null;
    }
    let mut text = String::new();
    let _ = stdin.read_to_string(&mut text);
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One helper command; never panics on bad input.
pub fn run(command: &str, input: &Value) -> Value {
    let Some(dir) = config::default_dir() else {
        return failure("Could not find the home folder for the deep search settings.");
    };
    let (config, prompts) = match config::load(&dir) {
        Ok(loaded) => loaded,
        Err(error) => return failure(&error.to_string()),
    };
    match command {
        "config" => json!({
            "ok": true,
            "config": config,
            "prompts": prompts,
            "guidance": Effort::ALL
                .map(|effort| (effort.as_str(), effort.guidance()))
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>(),
            "dir": dir,
        }),
        "search" => tool(&config, input, tools::web_search),
        "fetch" => tool(&config, input, tools::web_fetch),
        "check" => match serde_json::from_value::<CheckInput>(input.clone()) {
            Ok(input) => json!({ "ok": true, "checked": check::check(&config, &input) }),
            Err(error) => failure(&format!("Invalid input: {error}")),
        },
        "quotes" => match serde_json::from_value(input.clone()) {
            Ok(input) => json!({ "ok": true, "found": check::quotes(&input) }),
            Err(error) => failure(&format!("Invalid input: {error}")),
        },
        other => failure(&format!("Unknown deep search command \"{other}\".")),
    }
}

fn tool<I: serde::de::DeserializeOwned>(
    config: &Config,
    input: &Value,
    run: fn(&Config, &I) -> Result<tools::Output, String>,
) -> Value {
    let input: I = match serde_json::from_value(input.clone()) {
        Ok(input) => input,
        Err(error) => return failure(&format!("Invalid tool input: {error}")),
    };
    match run(config, &input) {
        Ok(output) => json!({ "ok": true, "text": output.text, "details": output.details }),
        Err(error) => failure(&error),
    }
}

fn failure(message: &str) -> Value {
    json!({ "ok": false, "error": message })
}
