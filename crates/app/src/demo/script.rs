//! Storyboards for demo mode: a TOML file of steps (`[[step]]` tables, `do = "<kind>"`).
//!
//! ```toml
//! window_title = "Pistachio Demo"
//! theme = "dark"
//! require_model = "inkling-2-mini"
//!
//! [[step]]
//! do = "mode"
//! mode = "code"
//!
//! [[step]]
//! do = "type"
//! text = "Hello"
//! cps = 25
//! ```

use std::path::{Path, PathBuf};

use pi_rpc::protocol::ThinkingLevel;
use serde::Deserialize;

/// Overrides the storyboard's `timeline` (recorders keep it with their other files).
pub const TIMELINE_ENV: &str = "PISTACHIO_DEMO_TIMELINE";
/// Overrides the storyboard's `start_file`.
pub const START_FILE_ENV: &str = "PISTACHIO_DEMO_START_FILE";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storyboard {
    /// Where the timeline (JSON lines) is written. Default: next to the storyboard, as
    /// `<name>.timeline.jsonl`.
    #[serde(default)]
    pub timeline: Option<PathBuf>,
    /// The window's title while the demo runs, so a recorder can find this window.
    #[serde(default = "default_title")]
    pub window_title: String,
    /// The theme to start in (not animated; use a `theme` step for that).
    #[serde(default)]
    pub theme: Option<ThemeName>,
    /// `send` refuses to send unless the chat uses this model (by id).
    #[serde(default)]
    pub require_model: Option<String>,
    /// Wait for this file to exist before the first step (a recorder creates it once it
    /// records). Steps start right away without it.
    #[serde(default)]
    pub start_file: Option<PathBuf>,
    #[serde(default, rename = "step")]
    pub steps: Vec<Step>,
}

/// One storyboard step. Steps that point at a control glide the demo cursor to it first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Do nothing for a while; with `until_idle`, stop early once the agent has finished.
    Wait {
        ms: u64,
        #[serde(default)]
        until_idle: bool,
    },
    /// Click the sidebar's Chat / Code tab.
    Mode { mode: ModeName },
    /// Start a session: in `folder` (Code), or a new chat. Waits until pi lists its models.
    NewChat {
        #[serde(default)]
        folder: Option<PathBuf>,
    },
    /// Choose a model (by id) in the composer's model picker.
    PickModel { id: String },
    /// Choose a thinking level in the composer's picker.
    Thinking { level: ThinkingLevel },
    /// Choose a web mode in a chat's globe menu.
    Web { mode: WebName },
    /// Type into the composer at a human pace (characters per second, slightly jittered).
    Type {
        text: String,
        #[serde(default = "default_cps")]
        cps: f32,
    },
    /// Click Send.
    Send,
    /// Wait until the agent has finished (it must start working first).
    WaitIdle {
        #[serde(default = "default_idle_timeout")]
        timeout_s: u64,
    },
    /// Click the context-window wheel to open its breakdown.
    OpenContext,
    /// Close the context breakdown.
    CloseContext,
    /// Click the header's Changes button (the right-hand diff panel).
    ShowDiff,
    /// Click a changed file in the diff panel to expand its patch.
    ExpandDiff { path: String },
    /// Scroll the diff panel down by `lines` over `ms`.
    ScrollDiff { lines: u32, ms: u64 },
    /// Click Settings in the sidebar.
    OpenSettings,
    /// Choose a theme in the Settings page's Theme menu (the page must be open).
    Theme { theme: ThemeName },
    /// Glide the cursor to a point, as fractions of the window.
    Move { x: f32, y: f32 },
    /// A named point in the timeline (for editing).
    Marker { name: String },
}

impl Step {
    /// The step's name in the timeline.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Wait { .. } => "wait",
            Self::Mode { .. } => "mode",
            Self::NewChat { .. } => "new_chat",
            Self::PickModel { .. } => "pick_model",
            Self::Thinking { .. } => "thinking",
            Self::Web { .. } => "web",
            Self::Type { .. } => "type",
            Self::Send => "send",
            Self::WaitIdle { .. } => "wait_idle",
            Self::OpenContext => "open_context",
            Self::CloseContext => "close_context",
            Self::ShowDiff => "show_diff",
            Self::ExpandDiff { .. } => "expand_diff",
            Self::ScrollDiff { .. } => "scroll_diff",
            Self::OpenSettings => "open_settings",
            Self::Theme { .. } => "theme",
            Self::Move { .. } => "move",
            Self::Marker { .. } => "marker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeName {
    Chat,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebName {
    Off,
    Search,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    Light,
    Dark,
}

fn default_title() -> String {
    "Pistachio Demo".to_owned()
}

fn default_cps() -> f32 {
    25.0
}

fn default_idle_timeout() -> u64 {
    900
}

impl Storyboard {
    /// Reads and checks a storyboard; relative paths in it are relative to its folder.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
        let mut storyboard =
            Self::parse(&text).map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let default_timeline = {
            let stem = path
                .file_stem()
                .map_or_else(|| "demo".into(), |stem| stem.to_string_lossy());
            PathBuf::from(format!("{stem}.timeline.jsonl"))
        };
        let timeline = std::env::var_os(TIMELINE_ENV)
            .map(PathBuf::from)
            .or_else(|| storyboard.timeline.take())
            .unwrap_or(default_timeline);
        storyboard.timeline = Some(base.join(timeline));
        storyboard.start_file = std::env::var_os(START_FILE_ENV)
            .map(PathBuf::from)
            .or_else(|| storyboard.start_file.take())
            .map(|file| base.join(file));
        for step in &mut storyboard.steps {
            if let Step::NewChat {
                folder: Some(folder),
            } = step
            {
                *folder = base.join(&*folder);
            }
        }
        Ok(storyboard)
    }

    /// Parses a storyboard and checks its values.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let storyboard: Self = toml::from_str(text)?;
        for (index, step) in storyboard.steps.iter().enumerate() {
            let problem = match step {
                Step::Type { text, cps } => (text.is_empty() || !cps.is_finite() || *cps <= 0.0)
                    .then_some("needs text and cps > 0"),
                Step::ScrollDiff { lines, ms } => {
                    (*lines == 0 || *ms == 0).then_some("needs lines and ms > 0")
                }
                Step::WaitIdle { timeout_s } => (*timeout_s == 0).then_some("needs timeout_s > 0"),
                Step::Move { x, y } => (!(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y))
                    .then_some("x and y are fractions of the window (0 to 1)"),
                _ => None,
            };
            if let Some(problem) = problem {
                anyhow::bail!("step {} ({}) {problem}", index + 1, step.kind());
            }
        }
        Ok(storyboard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_every_step_kind() {
        let storyboard = Storyboard::parse(
            r#"
            theme = "dark"
            require_model = "inkling-2-mini"

            [[step]]
            do = "wait"
            ms = 500
            [[step]]
            do = "wait"
            ms = 12000
            until_idle = true
            [[step]]
            do = "mode"
            mode = "code"
            [[step]]
            do = "new_chat"
            folder = "/tmp/demo/snake"
            [[step]]
            do = "new_chat"
            [[step]]
            do = "pick_model"
            id = "inkling-2-mini"
            [[step]]
            do = "thinking"
            level = "high"
            [[step]]
            do = "web"
            mode = "search"
            [[step]]
            do = "type"
            text = "Hi"
            [[step]]
            do = "send"
            [[step]]
            do = "wait_idle"
            [[step]]
            do = "open_context"
            [[step]]
            do = "close_context"
            [[step]]
            do = "show_diff"
            [[step]]
            do = "expand_diff"
            path = "src/main.rs"
            [[step]]
            do = "scroll_diff"
            lines = 40
            ms = 4000
            [[step]]
            do = "open_settings"
            [[step]]
            do = "theme"
            theme = "light"
            [[step]]
            do = "move"
            x = 0.5
            y = 0.25
            [[step]]
            do = "marker"
            name = "end"
            "#,
        )
        .unwrap();
        assert_eq!(storyboard.window_title, "Pistachio Demo");
        assert_eq!(storyboard.theme, Some(ThemeName::Dark));
        assert_eq!(storyboard.require_model.as_deref(), Some("inkling-2-mini"));
        assert_eq!(
            storyboard.steps,
            vec![
                Step::Wait {
                    ms: 500,
                    until_idle: false
                },
                Step::Wait {
                    ms: 12000,
                    until_idle: true
                },
                Step::Mode {
                    mode: ModeName::Code
                },
                Step::NewChat {
                    folder: Some("/tmp/demo/snake".into())
                },
                Step::NewChat { folder: None },
                Step::PickModel {
                    id: "inkling-2-mini".into()
                },
                Step::Thinking {
                    level: ThinkingLevel::High
                },
                Step::Web {
                    mode: WebName::Search
                },
                Step::Type {
                    text: "Hi".into(),
                    cps: 25.0
                },
                Step::Send,
                Step::WaitIdle { timeout_s: 900 },
                Step::OpenContext,
                Step::CloseContext,
                Step::ShowDiff,
                Step::ExpandDiff {
                    path: "src/main.rs".into()
                },
                Step::ScrollDiff {
                    lines: 40,
                    ms: 4000
                },
                Step::OpenSettings,
                Step::Theme {
                    theme: ThemeName::Light
                },
                Step::Move { x: 0.5, y: 0.25 },
                Step::Marker { name: "end".into() },
            ]
        );
    }

    #[test]
    fn rejects_unknown_steps_fields_and_values() {
        let error = |text: &str| format!("{:#}", Storyboard::parse(text).unwrap_err());
        assert!(error("[[step]]\ndo = \"dance\"").contains("dance"));
        assert!(error("[[step]]\ndo = \"wait\"\nms = 1\nseconds = 2").contains("seconds"));
        assert!(error("[[step]]\ndo = \"thinking\"\nlevel = \"extreme\"").contains("extreme"));
        assert!(error("[[step]]\ndo = \"type\"\ntext = \"a\"\ncps = 0").contains("step 1 (type)"));
        assert!(error("[[step]]\ndo = \"move\"\nx = 2\ny = 0").contains("fractions"));
        assert!(error("titel = \"typo\"").contains("titel"));
    }

    #[test]
    fn load_resolves_paths_next_to_the_storyboard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readme.toml");
        std::fs::write(
            &path,
            "start_file = \"go\"\n[[step]]\ndo = \"new_chat\"\nfolder = \"project\"\n",
        )
        .unwrap();
        let storyboard = Storyboard::load(&path).unwrap();
        if std::env::var_os(TIMELINE_ENV).is_none() {
            assert_eq!(
                storyboard.timeline,
                Some(dir.path().join("readme.timeline.jsonl"))
            );
        }
        if std::env::var_os(START_FILE_ENV).is_none() {
            assert_eq!(storyboard.start_file, Some(dir.path().join("go")));
        }
        assert_eq!(
            storyboard.steps,
            vec![Step::NewChat {
                folder: Some(dir.path().join("project"))
            }]
        );
    }
}
