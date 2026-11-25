//! The main window's drafts and renames, with a fake pi (a script).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::{App, AppContext as _, Entity, TestAppContext, VisualTestContext};
use pi_sessions::{Mode, SessionSummary};

use super::MainWindow;
use crate::chat::ComposerEvent;
use crate::chat::session::Lifecycle;
use crate::config::AppConfig;
use crate::hosts::{HostStatus, Hosts, HostsHandle};
use crate::panels::RightTab;
use crate::settings::AppSettings;
use crate::ui_state::UiState;
use crate::updates::Release;

fn fake_pi() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py")
}

/// [`fake_pi`], logging its arguments and what it is asked to `log`.
fn logging_pi(dir: &Path, log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let script = dir.join("pi");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"args: $*\" >> '{log}'\ntee -a '{log}' | '{fake}' \"$@\"\n",
            log = log.display(),
            fake = fake_pi().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The main window on the welcome screen, with `pi` as its pi and nothing read from or
/// written to the user's own settings.
fn open(
    pi: PathBuf,
    hosts: Vec<remote::SavedHost>,
    cx: &mut TestAppContext,
) -> (Entity<MainWindow>, &mut VisualTestContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        let mut config = AppConfig::for_tests(pi);
        // A scripted run: no update check, no "What's new" card, no reopened session.
        config.open_at_startup = vec!["code".into()];
        cx.set_global(config);
        cx.set_global(AppSettings {
            check_for_updates: false,
            ..AppSettings::default()
        });
        cx.set_global(UiState::default());
        assert_eq!(UiState::path(), None);
        crate::search_server::init(cx);
        crate::stt::init(cx);
        crate::naming::init(cx);
        let hosts = cx.new(|_| Hosts::for_tests(hosts));
        cx.set_global(HostsHandle(hosts));
        crate::chat::init(cx);
    });
    let mut window_view = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| MainWindow::new(window, cx));
        window_view = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.run_until_parked();
    (window_view.unwrap(), cx)
}

/// Does `f` to the window, as an event would.
fn act(
    window: &Entity<MainWindow>,
    cx: &mut VisualTestContext,
    f: impl FnOnce(&mut MainWindow, &mut gpui_kit::Window, &mut gpui_kit::Context<MainWindow>),
) {
    cx.update(|w, cx| window.update(cx, |this, cx| f(this, w, cx)));
    cx.run_until_parked();
}

fn type_text(window: &Entity<MainWindow>, text: &str, cx: &mut VisualTestContext) {
    act(window, cx, |this, window, cx| {
        let composer = this.chat.read(cx).composer().clone();
        composer.update(cx, |composer, cx| composer.set_text(text, window, cx));
    });
}

fn typed(window: &Entity<MainWindow>, cx: &VisualTestContext) -> String {
    cx.read(|cx| {
        let chat = window.read(cx).chat.read(cx);
        chat.composer().read(cx).text(cx)
    })
}

fn shown_folder(window: &Entity<MainWindow>, cx: &VisualTestContext) -> PathBuf {
    cx.read(|cx| window.read(cx).cwd(cx))
}

/// Waits (for real processes) until `done` holds.
fn wait(cx: &mut VisualTestContext, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if cx.read(&done) {
            return;
        }
        assert!(Instant::now() < deadline, "nothing happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_ready(window: &Entity<MainWindow>, cx: &mut VisualTestContext) {
    let window = window.clone();
    wait(cx, move |cx| {
        let chat = window.read(cx).chat.read(cx);
        chat.session().read(cx).lifecycle() == &Lifecycle::Ready
    });
}

fn summary(folder: &Path, file: &Path) -> SessionSummary {
    SessionSummary {
        path: file.to_path_buf(),
        id: "s1".into(),
        cwd: folder.to_path_buf(),
        name: None,
        first_prompt: Some("hi".into()),
        side_chat_of: None,
        inherited_prompts: None,
        modified: std::time::SystemTime::UNIX_EPOCH,
    }
}

/// A new session comes back, with what was typed in it, when its mode does; so does one in a
/// folder left for another, and switching modes never mixes them up.
#[gpui_kit::test]
fn a_new_session_comes_back_with_its_draft(cx: &mut TestAppContext) {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (window, cx) = open(fake_pi(), Vec::new(), cx);
    assert!(cx.read(|cx| window.read(cx).welcome_visible()));

    act(&window, cx, |this, window, cx| {
        this.new_chat_in(a.path().into(), window, cx);
    });
    type_text(&window, "draft in a ", cx);
    wait_ready(&window, cx);
    // Something shown in it (not sent) does not make it an old one.
    act(&window, cx, |this, _, cx| {
        let session = this.chat.read(cx).session().clone();
        session.update(cx, |session, cx| {
            session.push_notice(transcript::NoticeKind::Info, "a notice", cx);
            assert!(session.is_new() && !session.is_empty());
        });
    });
    act(&window, cx, |this, window, cx| {
        this.switch_to_mode(Mode::Chat, window, cx);
    });
    assert!(cx.read(|cx| window.read(cx).welcome_visible()));
    act(&window, cx, |this, window, cx| {
        this.switch_to_mode(Mode::Code, window, cx);
    });
    assert!(!cx.read(|cx| window.read(cx).welcome_visible()));
    assert_eq!(shown_folder(&window, cx), a.path());
    assert_eq!(typed(&window, cx), "draft in a ");

    // Another folder's new session has its own; "+" in the first brings the first back.
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(b.path().into(), window, cx);
    });
    assert_eq!(typed(&window, cx), "");
    type_text(&window, "draft in b", cx);
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(a.path().into(), window, cx);
    });
    assert_eq!(typed(&window, cx), "draft in a ");

    // It moves with the session when another folder is chosen in its composer, and the draft
    // that waits there keeps waiting.
    act(&window, cx, |this, _, cx| {
        let composer = this.chat.read(cx).composer().clone();
        composer.update(cx, |_, cx| {
            cx.emit(ComposerEvent::ChangeFolder(b.path().into()));
        });
    });
    assert_eq!(shown_folder(&window, cx), b.path());
    assert_eq!(typed(&window, cx), "draft in a ");
    type_text(&window, "", cx);
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(a.path().into(), window, cx);
    });
    assert_eq!(typed(&window, cx), "");
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(b.path().into(), window, cx);
    });
    assert_eq!(typed(&window, cx), "draft in b");

    // Without a draft, the new session itself still comes back.
    type_text(&window, "", cx);
    for mode in [Mode::Chat, Mode::Code] {
        act(&window, cx, |this, window, cx| {
            this.switch_to_mode(mode, window, cx);
        });
    }
    assert_eq!(shown_folder(&window, cx), b.path());
    assert_eq!(typed(&window, cx), "");
}

/// Each session keeps its own draft; deleting the session drops it.
#[gpui_kit::test]
fn sessions_keep_their_drafts(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (dir.path().join("1.jsonl"), dir.path().join("2.jsonl"));
    let (one, two) = (summary(dir.path(), &one), summary(dir.path(), &two));
    let (window, cx) = open(fake_pi(), Vec::new(), cx);
    let show = |session: &SessionSummary, cx: &mut VisualTestContext| {
        act(&window, cx, |this, window, cx| {
            drop(this.open_session(None, session, window, cx));
        });
    };
    show(&one, cx);
    type_text(&window, "for one", cx);
    show(&two, cx);
    assert_eq!(typed(&window, cx), "");
    type_text(&window, "for two", cx);
    show(&one, cx);
    assert_eq!(typed(&window, cx), "for one");
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(dir.path().into(), window, cx);
    });
    assert_eq!(typed(&window, cx), "");

    // Trashed as the window does, without the trash itself (it would move the files).
    cx.update(|_, cx| window.update(cx, |this, _| this.drafts.forget(None, &one.path)));
    show(&one, cx);
    assert_eq!(typed(&window, cx), "");
    show(&two, cx);
    assert_eq!(typed(&window, cx), "for two");
}

/// What is typed in a chat that then works in the background comes back with its session,
/// after the chat was let go.
#[gpui_kit::test]
fn a_finished_background_chat_keeps_its_draft(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let one = summary(dir.path(), &dir.path().join("1.jsonl"));
    let (window, cx) = open(fake_pi(), Vec::new(), cx);
    act(&window, cx, |this, window, cx| {
        let chat = this.open_session(None, &one, window, cx);
        let session = chat.read(cx).session().clone();
        session.update(cx, |session, cx| {
            session.submit("go".into(), Vec::new(), false, cx);
        });
    });
    let chat = cx.read(|cx| window.read(cx).chat.entity_id());
    cx.update(|w, cx| {
        window.update(cx, |this, cx| {
            let composer = this.chat.read(cx).composer().clone();
            composer.update(cx, |composer, cx| composer.set_text("next", w, cx));
            this.new_chat_in(dir.path().into(), w, cx);
            assert_eq!(this.background.len(), 1);
        });
    });
    wait(cx, |cx| window.read(cx).background.is_empty());
    assert_eq!(typed(&window, cx), "");
    act(&window, cx, |this, window, cx| {
        drop(this.open_session(None, &one, window, cx));
    });
    assert_ne!(cx.read(|cx| window.read(cx).chat.entity_id()), chat);
    assert_eq!(typed(&window, cx), "next");
}

/// Restarting pi keeps what is typed, also what the old pi gave back when it stopped.
#[gpui_kit::test]
fn restarting_pi_keeps_the_draft(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stop = dir.path().join("stop");
    let script = dir.path().join("pi");
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Never answers, and stops once told to.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nwhile [ ! -e '{}' ]; do sleep 0.02; done\n",
                stop.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (window, cx) = open(script, Vec::new(), cx);
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(dir.path().into(), window, cx);
        let session = this.chat.read(cx).session().clone();
        session.update(cx, |session, cx| {
            session.submit("sent too early".into(), Vec::new(), false, cx);
        });
    });
    type_text(&window, "draft", cx);
    std::fs::write(&stop, "").unwrap();
    wait(cx, |cx| {
        let chat = window.read(cx).chat.read(cx);
        matches!(
            chat.session().read(cx).lifecycle(),
            Lifecycle::Exited { .. }
        )
    });
    assert_eq!(typed(&window, cx), "sent too early\n\ndraft");
    let before = cx.read(|cx| window.read(cx).chat.entity_id());
    act(&window, cx, |this, window, cx| this.reconnect(window, cx));
    assert_ne!(cx.read(|cx| window.read(cx).chat.entity_id()), before);
    assert_eq!(shown_folder(&window, cx), dir.path());
    assert_eq!(typed(&window, cx), "sent too early\n\ndraft");
    assert!(cx.read(|cx| window.read(cx).background.is_empty()));
}

/// A message sent in a new session is not lost, nor the session, when its mode is left at
/// once; and what is typed while it works in the background is kept.
#[gpui_kit::test]
fn a_working_new_session_comes_back(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (window, cx) = open(fake_pi(), Vec::new(), cx);
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(dir.path().into(), window, cx);
        let session = this.chat.read(cx).session().clone();
        session.update(cx, |session, cx| {
            session.submit("go".into(), Vec::new(), false, cx);
        });
    });
    type_text(&window, "next", cx);
    let chat = cx.read(|cx| window.read(cx).chat.clone());
    // Before pi answers: the chat is busy with the message it has not sent.
    cx.update(|w, cx| {
        window.update(cx, |this, cx| this.switch_to_mode(Mode::Chat, w, cx));
        assert_eq!(window.read(cx).background.len(), 1);
        window.update(cx, |this, cx| this.switch_to_mode(Mode::Code, w, cx));
    });
    assert_eq!(cx.read(|cx| window.read(cx).chat.clone()), chat);
    assert_eq!(typed(&window, cx), "next");
}

/// Renaming a session that is not open starts a pi for it, sends the name and lets that pi go,
/// with the same chat and draft on screen throughout.
#[gpui_kit::test]
fn renaming_shows_nothing_else(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    let (window, cx) = open(logging_pi(dir.path(), &log), Vec::new(), cx);
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(dir.path().into(), window, cx);
    });
    type_text(&window, "draft", cx);
    let chat = cx.read(|cx| window.read(cx).chat.clone());
    let session = summary(dir.path(), &dir.path().join("s.jsonl"));
    act(&window, cx, |this, window, cx| {
        this.rename_session(None, &session, "A better name".into(), window, cx);
    });
    wait(cx, |cx| window.read(cx).renames.is_empty());
    let log_text = || std::fs::read_to_string(&log).unwrap_or_default();
    wait(cx, |_| log_text().contains("set_session_name"));
    let text = log_text();
    assert!(text.contains("A better name"), "{text}");
    assert!(text.contains("s.jsonl"), "{text}");
    assert_eq!(cx.read(|cx| window.read(cx).chat.clone()), chat);
    assert_eq!(typed(&window, cx), "draft");
    let notifications = |cx: &mut VisualTestContext| cx.update(|w, cx| w.notifications(cx).len());
    assert_eq!(notifications(cx), 0);

    // Open, it is renamed by its own pi: no other one starts.
    act(&window, cx, |this, window, cx| {
        drop(this.open_session(None, &session, window, cx));
    });
    wait_ready(&window, cx);
    let started = log_text().matches("args:").count();
    act(&window, cx, |this, window, cx| {
        this.rename_session(None, &session, "Once more".into(), window, cx);
    });
    wait(cx, |cx| window.read(cx).renames.is_empty());
    assert_eq!(log_text().matches("args:").count(), started);
    let title = cx.read(|cx| window.read(cx).chat.read(cx).session().read(cx).title());
    assert_eq!(title.as_deref(), Some("Once more"));

    // Opened while its rename is under way, it gets the name too.
    let third = summary(dir.path(), &dir.path().join("third.jsonl"));
    cx.update(|w, cx| {
        window.update(cx, |this, cx| {
            this.rename_session(None, &third, "Third".into(), w, cx);
            drop(this.open_session(None, &third, w, cx));
        });
    });
    wait(cx, |cx| window.read(cx).renames.is_empty());
    wait_ready(&window, cx);
    let title = cx.read(|cx| window.read(cx).chat.read(cx).session().read(cx).title());
    assert_eq!(title.as_deref(), Some("Third"));
    assert_eq!(notifications(cx), 0);

    // A pi that cannot start is reported.
    cx.update(|_, cx| cx.global_mut::<AppConfig>().pi_program = dir.path().join("missing"));
    let other = summary(dir.path(), &dir.path().join("other.jsonl"));
    act(&window, cx, |this, window, cx| {
        this.rename_session(None, &other, "Name".into(), window, cx);
    });
    wait(cx, |cx| window.read(cx).renames.is_empty());
    assert_eq!(notifications(cx), 1);
}

/// A session on a host that is not connected is not renamed, and nothing connects.
#[gpui_kit::test]
fn renaming_on_a_host_needs_its_connection(cx: &mut TestAppContext) {
    let mut store = remote::HostStore::default();
    let alias = "pistachio-test.invalid".to_owned();
    let id = store.add("Box".into(), remote::Target::Alias { alias });
    let (window, cx) = open(fake_pi(), store.hosts, cx);
    let session = summary(Path::new("/srv"), Path::new("/srv/s.jsonl"));
    act(&window, cx, |this, window, cx| {
        this.rename_session(Some(id.clone()), &session, "Name".into(), window, cx);
    });
    assert!(cx.read(|cx| window.read(cx).renames.is_empty()));
    cx.read(|cx| {
        let status = crate::hosts::hosts(cx).read(cx).status(&id);
        assert_eq!(status, HostStatus::Idle);
    });
    assert_eq!(cx.update(|w, cx| w.notifications(cx).len()), 1);
}

/// A file link shows its file whatever the right panel shows: closed, the diff, or another file.
#[gpui_kit::test]
fn file_links_open_one_after_another(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    for name in ["fib.rs", "fib.py"] {
        std::fs::write(folder.path().join(name), "text\n").unwrap();
    }
    let (window, cx) = open(fake_pi(), Vec::new(), cx);
    act(&window, cx, |this, window, cx| {
        this.new_chat_in(folder.path().into(), window, cx);
    });
    wait_ready(&window, cx);
    let click = |name: &str, cx: &mut VisualTestContext| {
        let link = crate::chat::OpenFileLink {
            path: name.to_owned(),
            line: None,
        };
        act(&window, cx, |this, window, cx| {
            this.open_file_link(&link, window, cx);
        });
        cx.read(|cx| {
            let this = window.read(cx);
            assert!(this.right_open, "{name} closed the panel");
            let panel = this.right_panel.read(cx);
            assert_eq!(panel.tab(), RightTab::Files);
            assert_eq!(panel.open_path(cx), Some(folder.path().join(name)));
        });
    };
    act(&window, cx, |this, _, cx| this.set_right_open(false, cx));
    click("fib.rs", cx);
    click("fib.py", cx);
    click("fib.py", cx);
    act(&window, cx, |this, _, cx| this.show_tab(RightTab::Diff, cx));
    click("fib.rs", cx);

    // The Files button still closes the panel.
    act(&window, cx, |this, _, cx| {
        this.show_tab(RightTab::Files, cx);
    });
    assert!(!cx.read(|cx| window.read(cx).right_open));
}

/// "Check for updates" shows one notification: that it checks, then the result in its place.
#[gpui_kit::test]
fn an_update_check_shows_one_notification(cx: &mut TestAppContext) {
    let (window, cx) = open(fake_pi(), Vec::new(), cx);
    // As `check_for_updates` does, without asking GitHub.
    let checking = |cx: &mut VisualTestContext| {
        act(&window, cx, |_, window, cx| {
            MainWindow::show_update_check(window, cx);
        });
    };
    let done = |result: Result<Option<Release>, String>, cx: &mut VisualTestContext| {
        act(&window, cx, |this, window, cx| {
            this.update_check_done(true, result, window, cx);
        });
    };
    let count = |cx: &mut VisualTestContext| cx.update(|w, cx| w.notifications(cx).len());
    for result in [Ok(None), Err("no network".to_owned())] {
        checking(cx);
        assert_eq!(count(cx), 1);
        done(result, cx);
        assert_eq!(count(cx), 1);
    }
    // The card with the offer stands alone.
    checking(cx);
    assert_eq!(count(cx), 1);
    let release = Release {
        version: "9999.0.0".into(),
        page_url: String::new(),
        download_url: None,
        notes: String::new(),
        assets: Vec::new(),
    };
    done(Ok(Some(release)), cx);
    let checked = window.clone();
    cx.read(|cx| assert!(checked.read(cx).update_prompt.is_some()));
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(count(cx), 0);
}
