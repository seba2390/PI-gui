//! Runs the bash labeller over real transcripts: pi session files (`toolCall` "bash") and
//! Claude Code project files (`tool_use` "Bash", with the model's own description for
//! comparison). Prints coverage and speed; `SHOW=labels`, `SHOW=raw` or `SHOW=why` also
//! lists each command's label, the commands left raw, or the part that was not understood.
//!
//! `cargo run --release -p transcript --example label_corpus -- ~/.pi/agent/sessions/*/*.jsonl`

use std::io::BufRead as _;
use std::time::Instant;

use serde_json::Value;
use transcript::command;

fn main() {
    let mut commands: Vec<(String, String)> = Vec::new();
    for path in std::env::args().skip(1) {
        let Ok(file) = std::fs::File::open(&path) else {
            eprintln!("cannot read {path}");
            continue;
        };
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(content) = value.pointer("/message/content").and_then(Value::as_array) else {
                continue;
            };
            for block in content {
                let (pointer, description) = match (block["type"].as_str(), block["name"].as_str())
                {
                    (Some("toolCall"), Some("bash")) => ("/arguments/command", None),
                    (Some("tool_use"), Some("Bash")) => {
                        ("/input/command", Some("/input/description"))
                    }
                    _ => continue,
                };
                if let Some(command) = block.pointer(pointer).and_then(Value::as_str) {
                    let description = description
                        .and_then(|pointer| block.pointer(pointer))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    commands.push((command.to_owned(), description.to_owned()));
                }
            }
        }
    }
    commands.sort();
    commands.dedup();

    let start = Instant::now();
    let results: Vec<_> = commands
        .iter()
        .map(|(command, _)| command::summarize(command))
        .collect();
    let elapsed = start.elapsed();
    let labelled = results.iter().flatten().count();
    let exploring = results
        .iter()
        .flatten()
        .filter(|summary| summary.is_exploration())
        .count();
    let per_command = elapsed / u32::try_from(commands.len().max(1)).unwrap_or(u32::MAX);
    println!(
        "{} commands, {labelled} labelled ({}%), {exploring} exploring; {elapsed:?} total, {per_command:?}/command",
        commands.len(),
        labelled * 100 / commands.len().max(1),
    );

    let show = std::env::var("SHOW").unwrap_or_default();
    for ((command, description), summary) in commands.iter().zip(&results) {
        let one_line: String = command.replace('\n', "⏎").chars().take(110).collect();
        match (show.as_str(), summary) {
            ("labels", Some(summary)) => println!(
                "{} {:<60} | {description:<40} ← {one_line}",
                if summary.is_exploration() { "E" } else { " " },
                summary.sentence(),
            ),
            ("raw", None) => println!("RAW {description} ← {one_line}"),
            ("why", None) => println!(
                "{}",
                command::unknown_part(command)
                    .unwrap_or_default()
                    .replace('\n', "⏎")
                    .chars()
                    .take(90)
                    .collect::<String>()
            ),
            _ => {}
        }
    }
}
