//! The helper the pi extension runs: `<app> --python-tool` with `{"code": …}` on stdin and one
//! JSON line on stdout: `{"output", "value", "error"}` (see [`crate::Outcome`]).
//!
//! By hand, the words after the flag are the code:
//! `pistachio --python-tool 'import math; math.log2(443)'`.

use std::io::{IsTerminal as _, Read as _};

use serde_json::{Value, json};

pub const FLAG: &str = "--python-tool";

/// Runs the helper when the process was started with [`FLAG`], returning its exit code.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(FLAG) {
        return None;
    }
    let words: Vec<String> = args.collect();
    // A crash inside the interpreter is an answer too, not a crash of the app.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}");
        println!(
            "\n{}",
            json!({ "output": "", "value": null, "error": "The interpreter failed on this code." })
        );
        std::process::exit(1);
    }));
    let by_hand = !words.is_empty();
    let code = if by_hand {
        words.join(" ")
    } else {
        read_stdin()["code"].as_str().unwrap_or_default().to_owned()
    };
    let outcome = if code.trim().is_empty() {
        crate::Outcome {
            output: String::new(),
            value: None,
            error: Some("No code was given.".to_owned()),
        }
    } else {
        crate::run(&code)
    };
    if by_hand {
        print!("{}", outcome.output);
        if let Some(value) = &outcome.value {
            println!("{value}");
        }
        if let Some(error) = &outcome.error {
            eprintln!("{error}");
        }
    } else {
        println!(
            "\n{}",
            json!({ "output": outcome.output, "value": outcome.value, "error": outcome.error })
        );
    }
    Some(i32::from(outcome.error.is_some()))
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
