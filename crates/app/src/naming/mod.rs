//! Automatic conversation names (Settings → Conversation names, off by default).
//!
//! When a new conversation's first message is sent, a short name is made from it and saved
//! with pi's `set_session_name` (so the terminal `pi` shows it too), unless the user named the
//! conversation first. The message goes to pi right away: naming runs beside it and never
//! holds the conversation up; the name arrives a moment later and the sidebar types it out.
//!
//! - [`Namer::Conversation`]: the conversation's own model, called once through a separate
//!   `pi --print --no-session` (see [`titles::print`]); the conversation's pi, its session
//!   file and the model's context never see the request. On an SSH host that pi runs on the
//!   host, the same way the conversation's does.
//! - [`Namer::Local`]: a small model from [`catalog`], downloaded on request and run on this
//!   computer (see [`titles::local`]).
//!
//! Very short first messages ("Fix the login bug") are their own name.

pub mod catalog;
pub mod ui;

use std::ffi::OsString;
use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global};
use remote::HostId;
use serde::{Deserialize, Serialize};
use titles::local::Arch;
use titles::print::Invocation;

use crate::config::AppConfig;
use crate::model_store::{ModelStore, StoreEvent};
use crate::settings::AppSettings;

/// What makes the names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Namer {
    /// The model the conversation uses.
    #[default]
    Conversation,
    /// A small model downloaded to this computer.
    Local,
}

impl Namer {
    pub const ALL: [Self; 2] = [Self::Conversation, Self::Local];

    pub fn id(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Local => "local",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Conversation => "The conversation's model",
            Self::Local => "A small model on this computer",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|namer| namer.id() == id)
            .unwrap_or_default()
    }
}

pub type NamingModels = ModelStore<catalog::Model>;
pub type NamingModelsEvent = StoreEvent;

/// A name was made for a conversation (the sidebar types it out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The SSH host the session lives on (`None`: this computer).
    pub host: Option<HostId>,
    pub path: PathBuf,
    pub title: String,
}

/// Announces [`Named`] events.
pub struct Names;

impl EventEmitter<Named> for Names {}

struct NamingHandle {
    models: Entity<NamingModels>,
    names: Entity<Names>,
}

impl Global for NamingHandle {}

pub fn init(cx: &mut App) {
    let dir = pi_sessions::app_dir().map(|dir| dir.join("naming"));
    let models = cx.new(|_| NamingModels::new(dir, catalog::MODELS));
    let names = cx.new(|_| Names);
    cx.set_global(NamingHandle { models, names });
}

pub fn models(cx: &App) -> Entity<NamingModels> {
    cx.global::<NamingHandle>().models.clone()
}

/// The [`Named`] announcer; `None` before [`init`] (tests).
pub fn names(cx: &App) -> Option<Entity<Names>> {
    cx.try_global::<NamingHandle>()
        .map(|handle| handle.names.clone())
}

/// Tells the sidebar that the session at `path` (on `host`) got `title`.
pub fn announce(host: Option<HostId>, path: Option<PathBuf>, title: String, cx: &mut App) {
    if let (Some(names), Some(path)) = (names(cx), path) {
        names.update(cx, |_, cx| cx.emit(Named { host, path, title }));
    }
}

/// The local model to name with: the chosen one if installed, else any installed one.
pub fn active_model(cx: &App) -> Option<&'static catalog::Model> {
    let handle = cx.try_global::<NamingHandle>()?;
    let models = handle.models.read(cx);
    let chosen = cx.global::<AppSettings>().naming_model.as_str();
    catalog::model(chosen)
        .filter(|model| models.is_installed(model.id))
        .or_else(|| {
            catalog::MODELS
                .iter()
                .find(|model| models.is_installed(model.id))
        })
}

/// How a conversation's pi is started, so the naming call runs the same way: on this
/// computer with the app's `pi` and environment, or on the SSH host through its shared
/// connection with the host's `pi`.
#[derive(Debug, Clone)]
pub struct PiLaunch {
    pub program: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    /// The conversation's folder (on the host, for a remote conversation): project settings
    /// there may define the model's provider.
    pub cwd: PathBuf,
    pub host: Option<remote::SavedHost>,
    /// An existing local folder for ssh to run in.
    pub local_dir: PathBuf,
}

impl PiLaunch {
    pub fn new(config: &AppConfig, cwd: PathBuf, host: Option<remote::SavedHost>) -> Self {
        Self {
            program: config.pi_program.clone(),
            env: config.child_env.vars().to_vec(),
            cwd,
            host,
            local_dir: config.home.clone().unwrap_or_else(std::env::temp_dir),
        }
    }

    /// The naming call for `provider`'s `model` (with pi's extensions or not).
    pub fn invocation(&self, provider: &str, model: &str, extensions: bool) -> Invocation {
        let args = titles::print::pi_args(provider, model, extensions);
        match &self.host {
            None => Invocation {
                program: self.program.clone(),
                args: args.into_iter().map(OsString::from).collect(),
                env: self.env.clone(),
                cwd: self.cwd.clone(),
            },
            Some(host) => {
                let argv: Vec<String> = std::iter::once(host.pi_command().to_owned())
                    .chain(args)
                    .collect();
                let ssh = remote::Ssh::new(host.target.clone());
                let (program, args) = ssh.command(Some(&self.cwd.to_string_lossy()), &argv);
                Invocation {
                    program,
                    args,
                    env: self.env.clone(),
                    cwd: self.local_dir.clone(),
                }
            }
        }
    }
}

/// One naming, ready to run off the main thread.
#[derive(Debug, Clone)]
pub enum Job {
    /// Through `pi --print` with the conversation's model.
    Pi {
        launch: Box<PiLaunch>,
        provider: String,
        model: String,
        message: String,
    },
    /// With a downloaded model.
    Local {
        arch: Arch,
        weights: PathBuf,
        tokenizer: PathBuf,
        message: String,
    },
}

/// The local naming job for `message`, if a local model is installed.
pub fn local_job(message: &str, cx: &App) -> Option<Job> {
    let model = active_model(cx)?;
    let dir = models(cx).read(cx).model_dir(model.id)?;
    Some(Job::Local {
        arch: model.arch,
        weights: dir.join(model.weights.file.name),
        tokenizer: dir.join(model.tokenizer.file.name),
        message: message.to_owned(),
    })
}

/// Runs `job` on its own thread; resolves to the name, or `None` (logged) when it failed.
pub async fn run(job: Job) -> Option<String> {
    let (sender, receiver) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("conversation-name".into())
        .spawn(move || {
            let result = match job {
                Job::Pi {
                    launch,
                    provider,
                    model,
                    message,
                } => titles::print::name_with_pi(
                    |extensions| launch.invocation(&provider, &model, extensions),
                    &message,
                    titles::print::TIMEOUT,
                ),
                Job::Local {
                    arch,
                    weights,
                    tokenizer,
                    message,
                } => titles::local::name(arch, &weights, &tokenizer, &message),
            };
            let _ = sender.send_blocking(result);
        });
    if let Err(error) = spawned {
        log::warn!("could not start naming the conversation: {error}");
        return None;
    }
    match receiver.recv().await {
        Ok(Ok(title)) => Some(title),
        Ok(Err(error)) => {
            log::warn!("naming the conversation failed: {error:#}");
            None
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Namer, PiLaunch};
    use crate::config::AppConfig;
    use std::path::PathBuf;

    #[test]
    fn namer_ids_round_trip() {
        for namer in Namer::ALL {
            assert_eq!(Namer::from_id(namer.id()), namer);
        }
        assert_eq!(Namer::from_id("?"), Namer::Conversation);
    }

    #[test]
    fn local_naming_runs_the_apps_pi_in_the_conversations_folder() {
        let config = AppConfig::for_tests("/usr/local/bin/pi".into());
        let launch = PiLaunch::new(&config, PathBuf::from("/work/app"), None);
        let invocation = launch.invocation("anthropic", "claude-x", false);
        assert_eq!(invocation.program, PathBuf::from("/usr/local/bin/pi"));
        assert_eq!(invocation.cwd, PathBuf::from("/work/app"));
        assert!(invocation.args.iter().any(|arg| arg == "--no-session"));
    }

    #[test]
    fn remote_naming_runs_the_hosts_pi_over_ssh() {
        let config = AppConfig::for_tests("pi".into());
        let host = remote::SavedHost {
            id: remote::HostId("box".into()),
            label: "Box".into(),
            target: remote::Target::Alias {
                alias: "box".into(),
            },
            pi_command: Some("/opt/pi/bin/pi".into()),
            recent_folders: Vec::new(),
        };
        let launch = PiLaunch::new(&config, PathBuf::from("/srv/my app"), Some(host));
        let invocation = launch.invocation("openai", "gpt-x", false);
        assert_eq!(invocation.program, PathBuf::from("ssh"));
        let joined = invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("/opt/pi/bin/pi"), "{joined}");
        assert!(
            joined.contains("--no-session") && joined.contains("gpt-x"),
            "{joined}"
        );
        assert!(joined.contains("'/srv/my app'"), "{joined}");
        assert_ne!(invocation.cwd, PathBuf::from("/srv/my app"));
    }
}
