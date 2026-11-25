//! The document parsers (the [`documents`] crate): the chosen one, which are installed, and the
//! download that may be running.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use documents::config::{self, Config};
use documents::{InstallError, Installed, Parser, Progress};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, Task, WeakEntity, Window,
};

use crate::config::AppConfig;

pub struct DocumentParsers {
    dir: PathBuf,
    config: Config,
    installed: HashMap<Parser, Installed>,
    download: Option<Download>,
    /// Why the last download or removal failed.
    errors: HashMap<Parser, String>,
}

struct Download {
    parser: Parser,
    progress: Option<Progress>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

#[derive(Debug, Clone)]
pub enum DocumentParsersEvent {
    /// The download finished, and it is the chosen one.
    Ready(Parser),
    Failed(String),
}

impl EventEmitter<DocumentParsersEvent> for DocumentParsers {}

struct DocumentParsersHandle(Entity<DocumentParsers>);

impl Global for DocumentParsersHandle {}

/// After [`AppConfig`] is set.
pub fn document_parsers(cx: &mut App) -> Entity<DocumentParsers> {
    if let Some(handle) = cx.try_global::<DocumentParsersHandle>() {
        return handle.0.clone();
    }
    let dir = cx.global::<AppConfig>().documents_dir.clone();
    let parsers = cx.new(|_| DocumentParsers::new(dir));
    cx.set_global(DocumentParsersHandle(parsers.clone()));
    parsers
}

/// [`document_parsers`], if anything asked for it before.
pub fn existing(cx: &App) -> Option<&DocumentParsers> {
    Some(cx.try_global::<DocumentParsersHandle>()?.0.read(cx))
}

/// A notification when a download ended.
pub fn notify(event: &DocumentParsersEvent, window: &mut Window, cx: &mut App) {
    let notification = match event {
        DocumentParsersEvent::Ready(parser) => Notification::success(format!(
            "{} is ready. Documents are read with it now.",
            parser.label()
        )),
        DocumentParsersEvent::Failed(error) => Notification::error(error.clone()),
    };
    window.push_notification(notification, cx);
}

impl DocumentParsers {
    fn new(dir: PathBuf) -> Self {
        let mut this = Self {
            dir,
            config: Config::default(),
            installed: HashMap::new(),
            download: None,
            errors: HashMap::new(),
        };
        this.look();
        this
    }

    fn look(&mut self) {
        self.config = config::load(&self.dir);
        self.installed = Parser::ALL
            .into_iter()
            .map(|parser| (parser, documents::installed(&self.dir, parser)))
            .collect();
    }

    /// Looks again at the settings and what is installed, e.g. when the Settings page opens.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.look();
        cx.notify();
    }

    /// The chosen parser.
    pub fn chosen(&self) -> Parser {
        self.config.parser
    }

    pub fn installed(&self, parser: Parser) -> Installed {
        self.installed
            .get(&parser)
            .copied()
            .unwrap_or(Installed::No)
    }

    pub fn error(&self, parser: Parser) -> Option<&str> {
        self.errors.get(&parser).map(String::as_str)
    }

    /// What is being downloaded, and how far that is, once known.
    pub fn downloading(&self) -> Option<(Parser, Option<Progress>)> {
        let download = self.download.as_ref()?;
        Some((download.parser, download.progress))
    }

    /// Reads documents with `parser`.
    pub fn choose(&mut self, parser: Parser, cx: &mut Context<Self>) {
        if self.config.parser == parser {
            return;
        }
        self.config.parser = parser;
        let (dir, config) = (self.dir.clone(), self.config.clone());
        let path = dir.join(config::CONFIG_FILE);
        crate::persist::save_in_background(cx, path, move |_| config::save(&dir, &config));
        cx.notify();
    }

    /// Downloads `parser` (again: an update), unless something is being downloaded.
    pub fn install(&mut self, parser: Parser, cx: &mut Context<Self>) {
        if self.download.is_some() {
            return;
        }
        self.errors.remove(&parser);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded();
        let thread = std::thread::Builder::new().name("parser-install".into());
        let spawned = thread.spawn({
            let (dir, cancel) = (self.dir.clone(), cancel.clone());
            move || {
                let result = documents::install(&dir, parser, &cancel, &|progress| {
                    let _ = tx.send_blocking(Ok(progress));
                });
                let _ = tx.send_blocking(Err(result));
            }
        });
        if let Err(error) = spawned {
            let error = format!("{} could not be downloaded: {error}", parser.label());
            return self.fail(parser, error, cx);
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
                    Err(result) => this.downloaded(parser, result, cx),
                });
            }
        });
        self.download = Some(Download {
            parser,
            progress: None,
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn downloaded(
        &mut self,
        parser: Parser,
        result: Result<(), InstallError>,
        cx: &mut Context<Self>,
    ) {
        self.download = None;
        self.installed
            .insert(parser, documents::installed(&self.dir, parser));
        match result {
            Ok(()) => {
                self.choose(parser, cx);
                cx.emit(DocumentParsersEvent::Ready(parser));
            }
            Err(InstallError::Cancelled) => {}
            Err(InstallError::Failed(error)) => self.fail(parser, error, cx),
        }
        cx.notify();
    }

    pub fn cancel_download(&mut self) {
        if let Some(download) = &self.download {
            download.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Deletes `parser`. If it was the chosen one, the built-in reader is.
    pub fn remove(&mut self, parser: Parser, cx: &mut Context<Self>) {
        if self.downloading().is_some_and(|(other, _)| other == parser) {
            return;
        }
        if self.config.parser == parser {
            self.choose(Parser::Builtin, cx);
        }
        self.installed.insert(parser, Installed::No);
        self.errors.remove(&parser);
        let dir = self.dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { documents::remove(&dir, parser) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    let error = format!("{} could not be removed: {error}", parser.label());
                    this.fail(parser, error, cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn fail(&mut self, parser: Parser, error: String, cx: &mut Context<Self>) {
        log::warn!("{error}");
        self.errors.insert(parser, error.clone());
        cx.emit(DocumentParsersEvent::Failed(error));
        cx.notify();
    }
}
