//! The `python` tool of chats: Python run in [Monty](https://github.com/pydantic/monty), an
//! interpreter written in Rust for code a model writes. It starts in microseconds and has no
//! files, network, environment or subprocesses: there is nothing to reach but the computation.
//! Chats use it to work numbers out instead of guessing them.
//!
//! The pi extension ([`EXTENSION_SOURCE`]) registers the tool and runs this crate's helper
//! ([`cli`]: the app's own executable started with [`cli::FLAG`]) for each call, so a crash of
//! the interpreter ends only that helper. [`run`] runs the code with limits on time, memory for
//! one allocation, recursion and printed output.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

pub mod cli;

/// The pi extension that registers the tool.
pub const EXTENSION_SOURCE: &str = include_str!("extension.ts");
/// The environment variable that tells the extension which executable runs the code.
pub const BIN_ENV: &str = "PISTACHIO_PYTHON_BIN";
/// The tool's name (in the allowlist chat sessions start pi with).
pub const TOOL_NAME: &str = "python";
/// The environment variable that tells the extension where [`SETTINGS_FILE`] is.
pub const DIR_ENV: &str = "PISTACHIO_PYTHON_DIR";
/// Whether chats may use the tool (`{"enabled": false}` turns it off). The extension reads it
/// before each message, so a change applies also to chats that are running.
pub const SETTINGS_FILE: &str = "settings.json";

/// How long the code may compute (sleeping not included).
const MAX_TIME: Duration = Duration::from_secs(5);
/// How long it may sleep in all.
const MAX_SLEEP: Duration = Duration::from_secs(1);
/// The largest single allocation: Monty checks sizes it can know up front (`[0] * 10**10`,
/// `2 ** 10**10`) against it. Growth step by step is bounded by the time instead, since the
/// running total needs Monty's allocator, which would wrap the whole app.
const MAX_MEMORY: usize = 256 * 1024 * 1024;
/// Recursion deeper than this is an error.
const MAX_DEPTH: usize = 400;
/// The most printed output kept.
const MAX_OUTPUT: usize = 64 * 1024;

/// Writes the extension into `dir` (when it changed) and returns its path.
pub fn install_extension(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("extension.ts");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION_SOURCE) {
        std::fs::write(&path, EXTENSION_SOURCE)?;
    }
    Ok(path)
}

/// Whether chats may use the tool: on unless [`SETTINGS_FILE`] in `dir` says otherwise.
pub fn enabled(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|settings| settings["enabled"].as_bool())
        .unwrap_or(true)
}

/// Turns the tool on or off for chats.
pub fn set_enabled(dir: &Path, on: bool) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string_pretty(&serde_json::json!({ "enabled": on }))?;
    std::fs::write(dir.join(SETTINGS_FILE), text + "\n")
}

/// What a run gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// What the code printed.
    pub output: String,
    /// The `repr` of the last expression's value, unless it is `None`.
    pub value: Option<String>,
    /// The error that ended the run (a traceback, like Python's).
    pub error: Option<String>,
}

/// Runs `code` with the limits above.
pub fn run(code: &str) -> Outcome {
    let mut output = String::new();
    let result = MontyRun::new(
        code.to_owned(),
        "main.py",
        Vec::new(),
        CompileOptions::default(),
    )
    .and_then(|mut runner| {
        let limits = ResourceLimits::default()
            .max_feed_duration(MAX_TIME)
            .max_total_sleep(MAX_SLEEP)
            .max_memory(MAX_MEMORY)
            .max_recursion_depth(MAX_DEPTH);
        runner.run(
            Vec::new(),
            ResourceTracker::new(limits),
            PrintWriter::CollectString(&mut output, Some(MAX_OUTPUT)),
        )
    });
    match result {
        Ok(value) => Outcome {
            output,
            value: (value != MontyObject::none()).then(|| value.py_repr()),
            error: None,
        },
        Err(error) => Outcome {
            output,
            value: None,
            error: Some(error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(code: &str) -> String {
        let outcome = run(code);
        assert_eq!(outcome.error, None, "{code}");
        outcome.value.unwrap_or_default()
    }

    #[test]
    fn the_switch_is_on_until_turned_off() {
        let dir = std::env::temp_dir().join(format!("python-tool-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(enabled(&dir), "on without a file");
        set_enabled(&dir, false).expect("write");
        assert!(!enabled(&dir));
        set_enabled(&dir, true).expect("write");
        assert!(enabled(&dir));
        std::fs::write(dir.join(SETTINGS_FILE), "not json").expect("write");
        assert!(enabled(&dir), "on when the file can't be read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn works_numbers_out() {
        assert_eq!(value("5 + 679"), "684");
        assert_eq!(value("import math\nround(math.log2(443), 6)"), "8.791163");
        assert_eq!(value("2 ** 100"), "1267650600228229401496703205376");
        let sum =
            value("import math\nsum(math.cos(i) * math.log(i) / 2 * i for i in range(1, 15))");
        let sum: f64 = sum.parse().expect("a number");
        let expected: f64 = (1..15)
            .map(|i| f64::from(i).cos() * f64::from(i).ln() / 2. * f64::from(i))
            .sum();
        assert!((sum - expected).abs() < 1e-12, "{sum} vs {expected}");
    }

    #[test]
    fn exact_decimals_and_statistics() {
        assert_eq!(
            value("from decimal import Decimal\nDecimal('0.1') + Decimal('0.2')"),
            "Decimal('0.3')"
        );
        assert_eq!(
            value("import statistics\nstatistics.median([3, 1, 2])"),
            "2"
        );
    }

    #[test]
    fn printed_output_and_none() {
        let outcome = run("print('a', 1)\nx = 2");
        assert_eq!(outcome.output, "a 1\n");
        assert_eq!(outcome.value, None);
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn errors_read_like_python() {
        let outcome = run("1 / 0");
        assert!(outcome.error.unwrap().contains("ZeroDivisionError"));
        assert!(run("def f(:\n").error.unwrap().contains("SyntaxError"));
    }

    #[test]
    fn nothing_outside_is_reachable() {
        for code in [
            "open('/etc/passwd').read()",
            "import os\nos.listdir('/')",
            "import os\nos.environ['HOME']",
            "import subprocess",
            "import socket",
            "import urllib.request",
        ] {
            let outcome = run(code);
            assert!(outcome.error.is_some(), "{code} ran: {outcome:?}");
            assert!(
                !outcome.value.unwrap_or_default().contains("root:"),
                "{code}"
            );
        }
    }

    #[test]
    fn limits_stop_runaway_code() {
        let huge = run("x = [0] * 10**10");
        assert!(
            huge.error.unwrap().contains("MemoryError"),
            "one huge allocation"
        );
        let power = run("2 ** (10 ** 12)");
        assert!(power.error.is_some(), "a power too large");
        let deep = run("def f(n):\n    return f(n + 1)\nf(0)");
        assert!(deep.error.unwrap().contains("RecursionError"));
    }

    #[test]
    fn a_loop_without_end_is_stopped() {
        let start = std::time::Instant::now();
        let outcome = run("while True:\n    pass");
        assert!(outcome.error.is_some());
        assert!(start.elapsed() < MAX_TIME + Duration::from_secs(2));
    }
}
