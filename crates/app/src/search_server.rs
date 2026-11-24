//! SearXNG on this computer (the [`searxng`] crate): downloading it, running it while chats need
//! it, and whether the one at the address in the settings can be used.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use deepsearch::config::BackendKind;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, Subscription, Task,
    WeakEntity, Window,
};
use searxng::{InstallError, Installed, Probe, Progress, Server};

use crate::config::AppConfig;

/// The running SearXNG, if any.
#[derive(Debug)]
pub enum Running {
    No,
    Starting,
    Here(Server),
    /// Another Pistachio on this computer runs it.
    Elsewhere,
}

pub struct SearchServer {
    root: PathBuf,
    installed: Installed,
    download: Option<Download>,
    running: Running,
    /// Why the last download or start failed.
    error: Option<String>,
    probe: Option<Check>,
    _quit: Subscription,
}

struct Download {
    progress: Option<Progress>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

/// What answers at `url`; `None` while that is not known.
struct Check {
    url: String,
    result: Option<Probe>,
    _task: Task<()>,
}

#[derive(Debug, Clone)]
pub enum SearchServerEvent {
    /// The download finished and it runs.
    Ready,
    /// The download was cancelled.
    Cancelled,
    Failed(String),
}

impl EventEmitter<SearchServerEvent> for SearchServer {}

struct SearchServerHandle(Entity<SearchServer>);

impl Global for SearchServerHandle {}

/// After [`AppConfig`] is set.
pub fn init(cx: &mut App) {
    let root = searxng::root_in(&cx.global::<AppConfig>().deepsearch_dir);
    let server = cx.new(|cx| SearchServer::new(root, cx));
    cx.set_global(SearchServerHandle(server));
}

pub fn search_server(cx: &App) -> Entity<SearchServer> {
    cx.global::<SearchServerHandle>().0.clone()
}

/// Starts SearXNG if chats are to search with the one on this computer and it does not run yet.
/// Called when a chat is about to search.
pub fn start_if_used(cx: &mut App) {
    let Some(server) = cx.try_global::<SearchServerHandle>() else {
        return;
    };
    let server = server.0.downgrade();
    let dir = cx.global::<AppConfig>().deepsearch_dir.clone();
    cx.spawn(async move |cx| {
        let used = cx
            .background_spawn(async move {
                deepsearch::config::load(&dir).is_ok_and(|(config, _)| {
                    config.backend == BackendKind::Searxng && config.searxng.local
                })
            })
            .await;
        if used {
            let _ = server.update(cx, |server, cx| server.start(cx));
        }
    })
    .detach();
}

/// A notification when the download ended or SearXNG could not start.
pub fn notify(event: &SearchServerEvent, window: &mut Window, cx: &mut App) {
    let notification = match event {
        SearchServerEvent::Ready => {
            Notification::success("SearXNG is running on this computer. Chats search with it now.")
        }
        SearchServerEvent::Failed(error) => Notification::error(error.clone()),
        SearchServerEvent::Cancelled => return,
    };
    window.push_notification(notification, cx);
}

impl SearchServer {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        // Not left to the end of the process: the port it was found by goes with it.
        let quit = cx.on_app_quit(|this, _| {
            this.running = Running::No;
            async {}
        });
        Self {
            installed: searxng::installed(&root),
            root,
            download: None,
            running: Running::No,
            error: None,
            probe: None,
            _quit: quit,
        }
    }

    pub fn installed(&self) -> Installed {
        self.installed
    }

    pub fn running(&self) -> &Running {
        &self.running
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_downloading(&self) -> bool {
        self.download.is_some()
    }

    /// How far the download is, once that is known.
    pub fn progress(&self) -> Option<Progress> {
        self.download.as_ref()?.progress
    }

    /// What answers at `url`, once that is known.
    pub fn probed(&self, url: &str) -> Option<Probe> {
        self.probe.as_ref().filter(|check| check.url == url)?.result
    }

    /// Looks again at what is installed and running, e.g. when the Settings page opens.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.download.is_none() {
            self.installed = searxng::installed(&self.root);
        }
        let gone = match &mut self.running {
            Running::Here(server) => !server.is_running(),
            Running::Elsewhere => true,
            Running::No => return self.look_elsewhere(cx),
            Running::Starting => false,
        };
        if gone {
            self.running = Running::No;
            self.look_elsewhere(cx);
        }
        cx.notify();
    }

    /// Notices the SearXNG another Pistachio runs.
    fn look_elsewhere(&self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { searxng::running_url(&root).is_some() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if found && matches!(this.running, Running::No) {
                    this.running = Running::Elsewhere;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads SearXNG (again: an update) and starts it.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        if self.download.is_some() || matches!(self.running, Running::Starting) {
            return;
        }
        // It must not run while its files are replaced.
        self.running = Running::No;
        self.error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded();
        let thread = std::thread::Builder::new().name("searxng-install".into());
        let spawned = thread.spawn({
            let (root, cancel) = (self.root.clone(), cancel.clone());
            move || {
                let result = searxng::install(&root, &cancel, &|progress| {
                    let _ = tx.send_blocking(Ok(progress));
                });
                let _ = tx.send_blocking(Err(result));
            }
        });
        if let Err(error) = spawned {
            return self.fail(format!("SearXNG could not be downloaded: {error}"), cx);
        }
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(message) = rx.recv().await {
                let _ = this.update(cx, |this, cx| match message {
                    Ok(progress) => {
                        if let Some(download) = &mut this.download {
                            download.progress = Some(progress);
                        }
                        cx.notify();
                    }
                    Err(result) => this.downloaded(result, cx),
                });
            }
        });
        self.download = Some(Download {
            progress: None,
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn downloaded(&mut self, result: Result<(), InstallError>, cx: &mut Context<Self>) {
        self.download = None;
        self.installed = searxng::installed(&self.root);
        match result {
            Ok(()) => self.start_and(true, cx),
            Err(InstallError::Cancelled) => cx.emit(SearchServerEvent::Cancelled),
            Err(InstallError::Failed(error)) => self.fail(error, cx),
        }
        cx.notify();
    }

    pub fn cancel_download(&mut self) {
        if let Some(download) = &self.download {
            download.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Starts SearXNG unless it runs (or is not installed).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.start_and(false, cx);
    }

    fn start_and(&mut self, announce: bool, cx: &mut Context<Self>) {
        let runs = match &mut self.running {
            Running::Starting => true,
            Running::Here(server) => server.is_running(),
            // May have stopped since.
            Running::Elsewhere | Running::No => false,
        };
        if runs || self.download.is_some() || self.installed == Installed::No {
            return;
        }
        self.running = Running::Starting;
        self.error = None;
        cx.notify();
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if searxng::running_url(&root).is_some() {
                        return Ok(Running::Elsewhere);
                    }
                    Server::start(&root).map(Running::Here)
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(running) => {
                    this.running = running;
                    if announce {
                        cx.emit(SearchServerEvent::Ready);
                    }
                    cx.notify();
                }
                Err(error) => {
                    this.running = Running::No;
                    this.fail(format!("SearXNG could not start: {error}"), cx);
                }
            });
        })
        .detach();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.running = Running::No;
        cx.notify();
    }

    /// Stops SearXNG and deletes everything that was downloaded.
    pub fn remove(&mut self, cx: &mut Context<Self>) {
        if self.download.is_some() || matches!(self.running, Running::Starting) {
            return;
        }
        self.running = Running::No;
        self.installed = Installed::No;
        self.error = None;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { searxng::remove(&root) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    this.fail(format!("SearXNG could not be removed: {error}"), cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Checks what answers at `url`; not again for the address that was checked last, unless
    /// `again`.
    pub fn probe(&mut self, url: &str, again: bool, cx: &mut Context<Self>) {
        if !again && self.probe.as_ref().is_some_and(|check| check.url == url) {
            return;
        }
        let address = url.to_owned();
        // Replacing it drops the check of another address.
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { searxng::probe(&address) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(check) = &mut this.probe {
                    check.result = Some(result);
                }
                cx.notify();
            });
        });
        self.probe = Some(Check {
            url: url.to_owned(),
            result: None,
            _task: task,
        });
        cx.notify();
    }

    /// Makes [`Self::probe`] check again.
    pub fn forget_probe(&mut self) {
        self.probe = None;
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        log::warn!("{error}");
        self.error = Some(error.clone());
        cx.emit(SearchServerEvent::Failed(error));
        cx.notify();
    }
}
