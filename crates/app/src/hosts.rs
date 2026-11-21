//! Saved SSH hosts and their live connection status (the device button's dot).
//!
//! Connections use the system `ssh` with a shared master connection per host (see the
//! `remote` crate), so every later command, including the pi process itself, reuses it. All
//! ssh calls block, so they run on the background executor; the UI only reads [`HostStatus`].

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::*;
use remote::{HostId, HostStore, Probe, RemoteError, SavedHost, Ssh, Target};

/// How often live connections are re-checked, so a dropped one turns its dot red.
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// What the device button shows for a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    /// Not connected yet (grey).
    Idle,
    /// Connecting and probing (amber).
    Connecting,
    /// Connected and pi was found (green).
    Connected,
    /// The host key is unknown; the user must confirm this fingerprint first.
    NeedsTrust { fingerprint: SharedString },
    /// Connected, but `pi` is not installed (or not on the login shell's PATH).
    MissingPi,
    /// Connecting failed (red); the message is shown in the tooltip and dialog.
    Failed(SharedString),
}

impl HostStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// The command that installs pi on a remote machine (shown when it is missing).
pub const INSTALL_PI: &str = "curl -fsSL https://pi.dev/install.sh | sh";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostsEvent {
    /// A host's status changed (or the list of hosts did).
    Changed,
}

pub struct Hosts {
    store: HostStore,
    status: HashMap<HostId, HostStatus>,
    probes: HashMap<HostId, Probe>,
    _checker: Task<()>,
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
            let ssh: Vec<Ssh> = this
                .connected()
                .map(|host| Ssh::new(host.target.clone()))
                .collect();
            async move {
                for ssh in ssh {
                    ssh.disconnect();
                }
            }
        })
        .detach();
        let checker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK_EVERY).await;
                let Ok(targets) = this.update(cx, |this, _| {
                    this.connected()
                        .map(|host| (host.id.clone(), host.target.clone()))
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                for (id, target) in targets {
                    let alive = cx
                        .background_executor()
                        .spawn(async move { Ssh::new(target).is_connected() })
                        .await;
                    if !alive {
                        let _ =
                            this.update(cx, |this, cx| this.set_status(&id, HostStatus::Idle, cx));
                    }
                }
            }
        });
        Self {
            store,
            status: HashMap::new(),
            probes: HashMap::new(),
            _checker: checker,
        }
    }

    fn connected(&self) -> impl Iterator<Item = &SavedHost> {
        self.store.hosts.iter().filter(|host| {
            matches!(
                self.status.get(&host.id),
                Some(HostStatus::Connected | HostStatus::MissingPi)
            )
        })
    }

    pub fn hosts(&self) -> &[SavedHost] {
        &self.store.hosts
    }

    pub fn get(&self, id: &HostId) -> Option<&SavedHost> {
        self.store.get(id)
    }

    pub fn status(&self, id: &HostId) -> HostStatus {
        self.status.get(id).cloned().unwrap_or(HostStatus::Idle)
    }

    pub fn probe(&self, id: &HostId) -> Option<&Probe> {
        self.probes.get(id)
    }

    /// An ssh handle for a saved host.
    pub fn ssh(&self, id: &HostId) -> Option<Ssh> {
        self.store.get(id).map(|host| Ssh::new(host.target.clone()))
    }

    /// Saves a new host (or returns the existing one with the same target).
    pub fn add(&mut self, label: String, target: Target, cx: &mut Context<Self>) -> HostId {
        let id = self.store.add(label, target);
        self.save(cx);
        cx.emit(HostsEvent::Changed);
        cx.notify();
        id
    }

    pub fn remove(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if let Some(ssh) = self.ssh(id) {
            cx.background_executor()
                .spawn(async move { ssh.disconnect() })
                .detach();
        }
        self.store.remove(id);
        self.status.remove(id);
        self.save(cx);
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
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = store.save(&path) {
                    log::warn!("could not save SSH hosts to {}: {error}", path.display());
                }
            })
            .detach();
    }

    fn set_status(&mut self, id: &HostId, status: HostStatus, cx: &mut Context<Self>) {
        if self.status.get(id) != Some(&status) {
            self.status.insert(id.clone(), status);
            cx.emit(HostsEvent::Changed);
            cx.notify();
        }
    }

    /// Connects (shared master connection) and checks for pi, in the background. With
    /// `trust_host_key`, an unknown host key is accepted (only after the user confirmed it).
    pub fn connect(&mut self, id: &HostId, trust_host_key: bool, cx: &mut Context<Self>) {
        let Some(host) = self.store.get(id).cloned() else {
            return;
        };
        if matches!(self.status(id), HostStatus::Connecting) {
            return;
        }
        self.set_status(id, HostStatus::Connecting, cx);
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let ssh = Ssh::new(host.target.clone());
            let pi = host.pi_command().to_owned();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match ssh.connect(trust_host_key) {
                        Ok(()) => ssh.probe(&pi).map(Ok),
                        Err(RemoteError::HostKeyUnknown) => {
                            let fingerprint = ssh
                                .host_key_fingerprint()
                                .unwrap_or_else(|error| error.to_string());
                            Ok(Err(fingerprint))
                        }
                        Err(error) => Err(error),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let status = match result {
                    Ok(Ok(probe)) => {
                        let status = if probe.pi_path.is_some() {
                            HostStatus::Connected
                        } else {
                            HostStatus::MissingPi
                        };
                        this.probes.insert(id.clone(), probe);
                        status
                    }
                    Ok(Err(fingerprint)) => HostStatus::NeedsTrust {
                        fingerprint: fingerprint.into(),
                    },
                    Err(error) => HostStatus::Failed(error.to_string().into()),
                };
                this.set_status(&id, status, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    /// End to end over a real SSH host, without installing anything there: pi-rpc's scripted
    /// fake pi is passed inline to the remote `python3`, started through the same
    /// `Ssh::command` + launcher path a remote session uses, and answers `get_state`.
    /// Run with `PI_GUI_SSH_TEST_HOST=<alias> cargo test -p pi-gui -- --ignored rpc_over_ssh`.
    #[test]
    #[ignore = "needs PI_GUI_SSH_TEST_HOST (an ssh alias with key auth and python3)"]
    fn rpc_over_ssh() {
        let Ok(alias) = std::env::var("PI_GUI_SSH_TEST_HOST") else {
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
        drop(process);
        ssh.disconnect();
    }
}
