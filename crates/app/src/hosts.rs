//! Saved SSH hosts: their live connection status (the device button's dot), automatic pi
//! installs, password / passphrase prompts, and the pi sessions stored on each host.
//!
//! Connections use the system `ssh` with a shared master connection per host (see the
//! `remote` crate), so every later command, including the pi process itself, reuses it. All
//! ssh calls block, so they run on the background executor; the UI only reads [`HostStatus`]
//! and the cached session lists.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::*;
use pi_sessions::SessionSummary;
use remote::{
    AskpassServer, HostId, HostStore, Probe, Prompt, RemoteError, SavedHost, SessionIndex, Ssh,
    Target,
};

/// How often live connections are re-checked (a dropped one turns its dot grey) and their
/// session lists refreshed.
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// What the device button shows for a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    /// Not connected yet (grey).
    Idle,
    /// Connecting and probing (amber).
    Connecting,
    /// Connected; installing pi there (amber). `step` says what is happening.
    Installing { step: SharedString },
    /// Connected and pi was found (green).
    Connected,
    /// The host key is unknown; the user must confirm this fingerprint first.
    NeedsTrust { fingerprint: SharedString },
    /// The host presented a different key than the recorded one.
    KeyChanged,
    /// Connected, but pi is not there: not installed yet (`error: None`, e.g. after a quiet
    /// connect at startup) or the install / custom pi command failed.
    MissingPi { error: Option<SharedString> },
    /// Connecting failed (red); the message is shown in the tooltip and dialog.
    Failed(SharedString),
}

impl HostStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Connected)
    }

    /// The shared connection is up (pi may still be missing).
    pub fn is_connected(&self) -> bool {
        matches!(
            self,
            Self::Connected | Self::MissingPi { .. } | Self::Installing { .. }
        )
    }

    /// Still working towards a result.
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Connecting | Self::Installing { .. })
    }
}

/// The command that installs pi by hand (shown when the automatic install failed).
pub const INSTALL_PI: &str = "curl -fsSL https://pi.dev/install.sh | sh";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostsEvent {
    /// A host's status changed (or the list of hosts did).
    Changed,
    /// The session list of this host changed.
    SessionsChanged(HostId),
    /// ssh asks the user something; take it with [`Hosts::take_prompt`].
    Prompt,
}

/// Sessions known on one host.
#[derive(Default)]
struct HostSessions {
    index: Arc<Mutex<SessionIndex>>,
    list: Vec<SessionSummary>,
    loaded: bool,
    error: Option<SharedString>,
}

pub struct Hosts {
    store: HostStore,
    status: HashMap<HostId, HostStatus>,
    probes: HashMap<HostId, Probe>,
    /// One `Ssh` per host, shared so remote [`workspace::Location`]s compare equal.
    ssh: HashMap<HostId, Arc<Ssh>>,
    sessions: HashMap<HostId, HostSessions>,
    refreshing: HashSet<HostId>,
    askpass: Option<AskpassServer>,
    prompts: VecDeque<Prompt>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<HostsEvent> for Hosts {}

/// The app-wide [`Hosts`] entity.
pub struct HostsHandle(pub Entity<Hosts>);

impl Global for HostsHandle {}

pub fn init(cx: &mut App) {
    let hosts = cx.new(Hosts::new);
    cx.set_global(HostsHandle(hosts));
}

pub fn hosts(cx: &App) -> Entity<Hosts> {
    cx.global::<HostsHandle>().0.clone()
}

impl Hosts {
    fn new(cx: &mut Context<Self>) -> Self {
        let store = HostStore::path()
            .map(|path| HostStore::load(&path))
            .unwrap_or_default();
        // Close shared connections when the app quits (they would linger for ControlPersist).
        cx.on_app_quit(|this, _| {
            let ssh: Vec<Arc<Ssh>> = this
                .store
                .hosts
                .iter()
                .filter(|host| this.status(&host.id).is_connected())
                .filter_map(|host| this.ssh.get(&host.id).cloned())
                .collect();
            async move {
                for ssh in ssh {
                    ssh.disconnect();
                }
            }
        })
        .detach();

        let (prompt_sender, prompt_receiver) = async_channel::unbounded::<Prompt>();
        let askpass = AskpassServer::start(&Ssh::default_control_dir(), prompt_sender)
            .map_err(|error| log::warn!("password prompts are unavailable: {error}"))
            .ok();
        let prompts = cx.spawn(async move |this, cx| {
            while let Ok(prompt) = prompt_receiver.recv().await {
                let pushed = this.update(cx, |this, cx| {
                    this.prompts.push_back(prompt);
                    cx.emit(HostsEvent::Prompt);
                });
                if pushed.is_err() {
                    break;
                }
            }
        });
        let checker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK_EVERY).await;
                let Ok(targets) = this.update(cx, |this, _| {
                    this.store
                        .hosts
                        .iter()
                        .filter(|host| this.status(&host.id).is_connected())
                        .filter_map(|host| Some((host.id.clone(), this.ssh.get(&host.id)?.clone())))
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                for (id, ssh) in targets {
                    let alive = cx
                        .background_executor()
                        .spawn(async move { ssh.is_connected() })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if alive {
                            this.refresh_sessions(&id, cx);
                        } else if this.status(&id).is_connected() {
                            this.set_status(&id, HostStatus::Idle, cx);
                        }
                    });
                }
            }
        });

        let mut this = Self {
            store,
            status: HashMap::new(),
            probes: HashMap::new(),
            ssh: HashMap::new(),
            sessions: HashMap::new(),
            refreshing: HashSet::new(),
            askpass,
            prompts: VecDeque::new(),
            _tasks: vec![prompts, checker],
        };
        // With "Connect saved SSH connections at start" (Settings), connect them quietly in
        // the background so their dots and sessions are there when needed. Quiet: no prompts
        // and no installs; failures only colour the dot.
        let at_start = cx
            .try_global::<crate::settings::AppSettings>()
            .is_some_and(|settings| settings.connect_hosts_at_start);
        if at_start {
            let ids: Vec<HostId> = this
                .store
                .hosts
                .iter()
                .map(|host| host.id.clone())
                .collect();
            for id in ids {
                this.connect_with(&id, ConnectMode::Quiet, cx);
            }
        }
        this
    }

    /// Connects `id` in the background without prompts or installs (e.g. to reopen its last
    /// session at start); failures only colour its dot.
    pub fn connect_quietly(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if !self.status(id).is_connected() {
            self.connect_with(id, ConnectMode::Quiet, cx);
        }
    }

    pub fn hosts(&self) -> &[SavedHost] {
        &self.store.hosts
    }

    pub fn get(&self, id: &HostId) -> Option<&SavedHost> {
        self.store.get(id)
    }

    pub fn label(&self, id: &HostId) -> String {
        self.get(id)
            .map_or_else(|| id.0.clone(), |host| host.label.clone())
    }

    pub fn status(&self, id: &HostId) -> HostStatus {
        self.status.get(id).cloned().unwrap_or(HostStatus::Idle)
    }

    pub fn probe(&self, id: &HostId) -> Option<&Probe> {
        self.probes.get(id)
    }

    /// The ssh handle of a saved host (shared; cheap to clone).
    pub fn ssh(&self, id: &HostId) -> Option<Arc<Ssh>> {
        if let Some(ssh) = self.ssh.get(id) {
            return Some(ssh.clone());
        }
        self.store
            .get(id)
            .map(|host| Arc::new(Ssh::new(host.target.clone())))
    }

    fn ssh_mut(&mut self, id: &HostId) -> Option<Arc<Ssh>> {
        let target = self.store.get(id)?.target.clone();
        Some(
            self.ssh
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Ssh::new(target)))
                .clone(),
        )
    }

    /// Where a folder on this host lives, for the diff / files panels and the file picker.
    pub fn location(&mut self, id: &HostId) -> workspace::Location {
        match self.ssh_mut(id) {
            Some(ssh) => workspace::Location::Remote(ssh),
            None => workspace::Location::Local,
        }
    }

    /// The pi command to start on the host: the path the probe found, else the saved one.
    pub fn pi_command(&self, id: &HostId) -> Option<String> {
        let host = self.get(id)?;
        Some(
            self.probe(id)
                .and_then(|probe| probe.pi_path.clone())
                .unwrap_or_else(|| host.pi_command().to_owned()),
        )
    }

    /// Sessions stored on the host (newest first), as of the last refresh.
    pub fn sessions(&self, id: &HostId) -> &[SessionSummary] {
        self.sessions.get(id).map_or(&[], |sessions| &sessions.list)
    }

    /// Whether the host's sessions were listed at least once.
    pub fn sessions_loaded(&self, id: &HostId) -> bool {
        self.sessions
            .get(id)
            .is_some_and(|sessions| sessions.loaded)
    }

    pub fn sessions_error(&self, id: &HostId) -> Option<SharedString> {
        self.sessions
            .get(id)
            .and_then(|sessions| sessions.error.clone())
    }

    /// The next question from ssh, if any.
    pub fn take_prompt(&mut self) -> Option<Prompt> {
        self.prompts.pop_front()
    }

    // ---- managing hosts ------------------------------------------------------

    /// Saves a new host (or returns the existing one with the same target).
    pub fn add(&mut self, label: String, target: Target, cx: &mut Context<Self>) -> HostId {
        let id = self.store.add(label, target);
        self.save(cx);
        cx.emit(HostsEvent::Changed);
        cx.notify();
        id
    }

    pub fn remove(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if let Some(ssh) = self.ssh.remove(id).or_else(|| self.ssh(id)) {
            cx.background_executor()
                .spawn(async move { ssh.disconnect() })
                .detach();
        }
        self.store.remove(id);
        self.status.remove(id);
        self.probes.remove(id);
        self.sessions.remove(id);
        self.save(cx);
        cx.emit(HostsEvent::Changed);
        cx.emit(HostsEvent::SessionsChanged(id.clone()));
        cx.notify();
    }

    pub fn rename(&mut self, id: &HostId, label: String, cx: &mut Context<Self>) {
        let label = label.trim().to_owned();
        if label.is_empty() {
            return;
        }
        if let Some(host) = self.store.get_mut(id) {
            host.label = label;
            self.save(cx);
            cx.emit(HostsEvent::Changed);
            cx.notify();
        }
    }

    /// Sets the command that starts pi on the host (empty: `pi`) and checks it again.
    pub fn set_pi_command(&mut self, id: &HostId, command: String, cx: &mut Context<Self>) {
        let command = Some(command.trim().to_owned()).filter(|command| !command.is_empty());
        let Some(host) = self.store.get_mut(id) else {
            return;
        };
        if host.pi_command == command {
            return;
        }
        host.pi_command = command;
        self.probes.remove(id);
        self.save(cx);
        if self.status(id).is_connected() {
            self.status.remove(id);
            self.connect(id, false, cx);
        }
        cx.emit(HostsEvent::Changed);
        cx.notify();
    }

    /// Remembers `folder` as the host's most recent folder.
    pub fn touch_folder(&mut self, id: &HostId, folder: &str, cx: &mut Context<Self>) {
        self.store.touch_folder(id, folder);
        self.save(cx);
    }

    fn save(&self, cx: &mut Context<Self>) {
        let (Some(path), store) = (HostStore::path(), self.store.clone()) else {
            return;
        };
        crate::persist::save_in_background(cx, path, move |path| store.save(path));
    }

    fn set_status(&mut self, id: &HostId, status: HostStatus, cx: &mut Context<Self>) {
        if self.status.get(id) != Some(&status) {
            self.status.insert(id.clone(), status);
            cx.emit(HostsEvent::Changed);
            cx.notify();
        }
    }

    // ---- connecting ----------------------------------------------------------

    /// Connects (shared master connection), checks for pi and installs it when missing, in
    /// the background. With `trust_host_key`, an unknown host key is accepted (only after the
    /// user confirmed it). Asks for passwords / passphrases in the app when keys alone fail.
    pub fn connect(&mut self, id: &HostId, trust_host_key: bool, cx: &mut Context<Self>) {
        let mode = if trust_host_key {
            ConnectMode::Trust
        } else {
            ConnectMode::Interactive
        };
        self.connect_with(id, mode, cx);
    }

    /// Checks right away whether the host's shared connection is still up (e.g. after a
    /// remote pi ended), so its dot and the session's banner tell the truth.
    pub fn check_connection(&mut self, id: &HostId, cx: &mut Context<Self>) {
        let Some(ssh) = self.ssh_mut(id) else { return };
        if !self.status(id).is_connected() {
            return;
        }
        let id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let alive = cx
                .background_executor()
                .spawn(async move { ssh.is_connected() })
                .await;
            if !alive {
                let _ = this.update(cx, |this, cx| {
                    if this.status(&id).is_connected() {
                        this.set_status(&id, HostStatus::Idle, cx);
                    }
                });
            }
        });
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(task);
    }

    /// Installs pi on a connected host (again), e.g. after a failed attempt.
    pub fn install(&mut self, id: &HostId, cx: &mut Context<Self>) {
        let Some(ssh) = self.ssh_mut(id) else { return };
        if self.status(id).is_busy() {
            return;
        }
        let id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let ok = Self::install_and_probe(this.clone(), id.clone(), ssh, cx).await;
            let _ = this.update(cx, |this, cx| {
                if ok {
                    this.refresh_sessions(&id, cx);
                }
            });
        });
        self._tasks.push(task);
    }

    fn connect_with(&mut self, id: &HostId, mode: ConnectMode, cx: &mut Context<Self>) {
        let Some(host) = self.store.get(id).cloned() else {
            return;
        };
        let Some(ssh) = self.ssh_mut(id) else { return };
        if self.status(id).is_busy() {
            return;
        }
        self.set_status(id, HostStatus::Connecting, cx);
        let askpass_env = match (&self.askpass, std::env::current_exe()) {
            (Some(server), Ok(exe)) if mode != ConnectMode::Quiet => Some(server.env(&exe)),
            _ => None,
        };
        let id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let pi = host.pi_command().to_owned();
            let probe_pi = pi.clone();
            let worker = ssh.clone();
            let result =
                cx.background_executor()
                    .spawn(async move {
                        let trust = mode == ConnectMode::Trust;
                        let connected = match worker.connect(trust) {
                            Err(RemoteError::AuthFailed(_)) if askpass_env.is_some() => worker
                                .connect_interactive(askpass_env.as_deref().unwrap_or(&[]), trust),
                            other => other,
                        };
                        match connected {
                            Ok(()) => worker.probe(&probe_pi).map(Connected::Probe),
                            // Only a key that was actually fetched can be confirmed.
                            Err(RemoteError::HostKeyUnknown) => {
                                worker.host_key_fingerprint().map(Connected::Untrusted)
                            }
                            Err(RemoteError::HostKeyChanged) => Ok(Connected::KeyChanged),
                            Err(error) => Err(error),
                        }
                    })
                    .await;
            let install = this
                .update(cx, |this, cx| {
                    let (status, install) = match result {
                        Ok(Connected::Probe(probe)) => {
                            let found = probe.pi_path.is_some();
                            this.probes.insert(id.clone(), probe);
                            if found {
                                (HostStatus::Connected, false)
                            } else if host.pi_command.is_some() {
                                (
                                    HostStatus::MissingPi {
                                        error: Some(
                                            format!("`{pi}` was not found on {}.", host.label)
                                                .into(),
                                        ),
                                    },
                                    false,
                                )
                            } else {
                                // Like an editor's remote server: install pi when it is used.
                                (
                                    HostStatus::MissingPi { error: None },
                                    mode != ConnectMode::Quiet,
                                )
                            }
                        }
                        Ok(Connected::Untrusted(fingerprint)) => (
                            HostStatus::NeedsTrust {
                                fingerprint: fingerprint.into(),
                            },
                            false,
                        ),
                        Ok(Connected::KeyChanged) => (HostStatus::KeyChanged, false),
                        Err(error) => (HostStatus::Failed(error.to_string().into()), false),
                    };
                    this.set_status(&id, status.clone(), cx);
                    if status.is_ready() {
                        this.refresh_sessions(&id, cx);
                    }
                    install
                })
                .unwrap_or(false);
            if install && Self::install_and_probe(this.clone(), id.clone(), ssh, cx).await {
                let _ = this.update(cx, |this, cx| this.refresh_sessions(&id, cx));
            }
        });
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(task);
    }

    /// Installs pi into `~/.pistachio/server` on the host, showing progress in the status, then
    /// probes again. Returns whether pi is now available.
    async fn install_and_probe(
        this: WeakEntity<Self>,
        id: HostId,
        ssh: Arc<Ssh>,
        cx: &mut AsyncApp,
    ) -> bool {
        let starting = HostStatus::Installing {
            step: "Preparing".into(),
        };
        if this
            .update(cx, |this, cx| this.set_status(&id, starting, cx))
            .is_err()
        {
            return false;
        }
        let (steps, step_receiver) = async_channel::unbounded::<String>();
        let progress = {
            let (this, id) = (this.clone(), id.clone());
            cx.spawn(async move |cx| {
                while let Ok(step) = step_receiver.recv().await {
                    let status = HostStatus::Installing { step: step.into() };
                    if this
                        .update(cx, |this, cx| this.set_status(&id, status, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        };
        let pi = this
            .read_with(cx, |this, _| {
                this.get(&id).map(|host| host.pi_command().to_owned())
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| "pi".to_owned());
        let result = cx
            .background_executor()
            .spawn(async move {
                ssh.install_pi(|step| {
                    let _ = steps.send_blocking(step.to_owned());
                })?;
                ssh.probe(&pi)
            })
            .await;
        drop(progress);
        this.update(cx, |this, cx| {
            let status = match result {
                Ok(probe) if probe.pi_path.is_some() => {
                    this.probes.insert(id.clone(), probe);
                    HostStatus::Connected
                }
                Ok(probe) => {
                    this.probes.insert(id.clone(), probe);
                    HostStatus::MissingPi {
                        error: Some("pi was installed but could not be started.".into()),
                    }
                }
                Err(error) => HostStatus::MissingPi {
                    error: Some(format!("Installing pi failed: {error}").into()),
                },
            };
            let ready = status.is_ready();
            this.set_status(&id, status, cx);
            ready
        })
        .unwrap_or(false)
    }

    /// Removes the old key of a host whose key changed (after the user confirmed the change
    /// is expected), then connects again, which asks to trust the new key.
    pub fn replace_host_key(&mut self, id: &HostId, cx: &mut Context<Self>) {
        let Some(ssh) = self.ssh_mut(id) else { return };
        self.set_status(id, HostStatus::Connecting, cx);
        let id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ssh.remove_known_host() })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(removed) => {
                    log::info!("removed the old host key of {removed}");
                    this.status.remove(&id);
                    this.connect(&id, false, cx);
                }
                Err(error) => this.set_status(
                    &id,
                    HostStatus::Failed(format!("Could not remove the old key: {error}").into()),
                    cx,
                ),
            });
        });
        self._tasks.push(task);
    }

    // ---- sessions ------------------------------------------------------------

    /// Re-lists the host's sessions in the background (only changed files are read).
    pub fn refresh_sessions(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if !self.status(id).is_connected() || self.refreshing.contains(id) {
            return;
        }
        let (Some(ssh), Some(dir)) = (
            self.ssh_mut(id),
            self.probe(id).map(|probe| probe.sessions_dir.clone()),
        ) else {
            return;
        };
        let index = self.sessions.entry(id.clone()).or_default().index.clone();
        self.refreshing.insert(id.clone());
        let id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut index = index
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    index.refresh(&ssh, &dir)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing.remove(&id);
                let entry = this.sessions.entry(id.clone()).or_default();
                match result {
                    Ok(list) => {
                        let changed = !entry.loaded || entry.list != list;
                        entry.list = list;
                        entry.loaded = true;
                        entry.error = None;
                        if changed {
                            cx.emit(HostsEvent::SessionsChanged(id.clone()));
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        log::debug!("could not list sessions on {}: {error}", id.0);
                        entry.error = Some(error.to_string().into());
                        cx.emit(HostsEvent::SessionsChanged(id.clone()));
                        cx.notify();
                    }
                }
            });
        });
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(task);
    }

    /// Moves a session file on the host to its trash, then refreshes the list.
    pub fn trash_session(
        &mut self,
        id: &HostId,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.trash_sessions(id, vec![path], cx)
    }

    /// Moves session files on the host to its trash (one round trip each, in the
    /// background), then refreshes the list. Reports the first failure.
    pub fn trash_sessions(
        &mut self,
        id: &HostId,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Some(ssh) = self.ssh_mut(id) else {
            return Task::ready(Err("Unknown host".to_owned()));
        };
        let texts: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        if let Some(sessions) = self.sessions.get_mut(id) {
            sessions
                .list
                .retain(|session| !paths.contains(&session.path));
            if let Ok(mut index) = sessions.index.lock() {
                for text in &texts {
                    index.forget(text);
                }
            }
        }
        cx.emit(HostsEvent::SessionsChanged(id.clone()));
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut first_error = None;
                    for text in &texts {
                        if let Err(error) = ssh.trash(text) {
                            first_error.get_or_insert(error.to_string());
                        }
                    }
                    first_error.map_or(Ok(()), Err)
                })
                .await;
            let _ = this.update(cx, |this, cx| this.refresh_sessions(&id, cx));
            result
        })
    }

    // ---- model settings ------------------------------------------------------

    /// Copies this computer's pi model settings (`models.json`, and `auth.json` only with
    /// `include_credentials`) to the host's agent folder. Existing files there are kept as
    /// `*.bak-pistachio`.
    pub fn copy_model_settings(
        &mut self,
        id: &HostId,
        include_credentials: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let (Some(ssh), Some(remote_agent)) = (
            self.ssh_mut(id),
            self.probe(id).map(|probe| probe.agent_dir.clone()),
        ) else {
            return Task::ready(Err("Connect to the host first.".to_owned()));
        };
        let Some(local_agent) = pi_sessions::agent_dir() else {
            return Task::ready(Err("pi's settings folder was not found here.".to_owned()));
        };
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut files = vec!["models.json"];
                    if include_credentials {
                        files.push("auth.json");
                    }
                    for name in files {
                        let local = local_agent.join(name);
                        let bytes = std::fs::read(&local).map_err(|error| {
                            format!("Could not read {}: {error}", local.display())
                        })?;
                        ssh.upload(&format!("{remote_agent}/{name}"), bytes)
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if result.is_ok()
                    && let Some(probe) = this.probes.get_mut(&id)
                {
                    probe.has_models = true;
                    cx.emit(HostsEvent::Changed);
                    cx.notify();
                }
            });
            result
        })
    }
}

/// How a connection attempt may interact with the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectMode {
    /// Background (startup): no prompts, no install.
    Quiet,
    /// The user asked: password prompts and automatic install allowed.
    Interactive,
    /// Like `Interactive`, accepting an unknown host key the user confirmed.
    Trust,
}

enum Connected {
    Probe(Probe),
    Untrusted(String),
    KeyChanged,
}

#[cfg(test)]
mod tests {
    /// ssh asks for a password through the app's own askpass (this crate's binary started by
    /// ssh); the test cancels, so no password is sent. Needs a host that offers password
    /// login: `PISTACHIO_SSH_PASSWORD_HOST=<alias> cargo build && cargo test -p pistachio -- \
    /// --ignored askpass_prompt_reaches_the_app`.
    #[test]
    #[ignore = "needs PISTACHIO_SSH_PASSWORD_HOST and the built pistachio binary"]
    fn askpass_prompt_reaches_the_app() {
        let Ok(alias) = std::env::var("PISTACHIO_SSH_PASSWORD_HOST") else {
            return;
        };
        // The app binary next to this test's build folders (`target/debug/pistachio`).
        let test = std::env::current_exe().unwrap();
        let exe = test
            .ancestors()
            .map(|dir| dir.join("pistachio"))
            .find(|path| path.is_file())
            .unwrap_or_default();
        assert!(exe.exists(), "build the app first: {}", exe.display());
        let dir = tempfile::Builder::new()
            .prefix("ap")
            .tempdir_in("/tmp")
            .unwrap();
        let (sender, prompts) = async_channel::unbounded();
        let server = remote::AskpassServer::start(dir.path(), sender).unwrap();
        let ssh =
            remote::Ssh::with_control_dir(remote::Target::Alias { alias }, dir.path().join("cm"));
        let answerer = std::thread::spawn(move || {
            let prompt = prompts.recv_blocking().expect("ssh asked through the app");
            eprintln!("prompt: {:?} (secret: {})", prompt.text, prompt.is_secret());
            assert!(
                prompt.text.to_lowercase().contains("password"),
                "{}",
                prompt.text
            );
            assert!(prompt.is_secret());
            prompt.answer(None);
        });
        let result = ssh.connect_interactive(&server.env(&exe), false);
        eprintln!("connect after cancelling: {result:?}");
        answerer.join().unwrap();
        assert!(result.is_err());
    }

    /// End to end over a real SSH host, without installing anything there: pi-rpc's scripted
    /// fake pi is passed inline to the remote `python3`, started through the same
    /// `Ssh::command` + launcher path a remote session uses, and answers `get_state`.
    /// Run with `PISTACHIO_SSH_TEST_HOST=<alias> cargo test -p pistachio -- --ignored rpc_over_ssh`.
    #[test]
    #[ignore = "needs PISTACHIO_SSH_TEST_HOST (an ssh alias with key auth and python3)"]
    fn rpc_over_ssh() {
        let Ok(alias) = std::env::var("PISTACHIO_SSH_TEST_HOST") else {
            return;
        };
        let ssh = remote::Ssh::new(remote::Target::Alias { alias });
        ssh.connect(false).expect("connect");
        let script = include_str!("../../pi-rpc/tests/fake_pi.py");
        let argv = ["python3", "-u", "-c", script, "--mode", "rpc"].map(str::to_owned);
        let (program, args) = ssh.command(Some("~"), &argv);
        let mut options = pi_rpc::SpawnOptions::new(std::env::temp_dir());
        options.launcher = Some(pi_rpc::Launcher { program, args });
        let (process, _events) = pi_rpc::PiProcess::spawn(&options).expect("spawn ssh");
        let state =
            futures::executor::block_on(process.client().get_state()).expect("get_state over ssh");
        assert_eq!(state.session_id, "s1");
        // The shared connection is left to expire (ControlPersist): other tests use it too.
        drop(process);
    }
}
