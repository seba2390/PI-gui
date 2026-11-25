//! Drives a real [`PiSession`] against the scripted fake pi from `pi-rpc`'s tests: prompts pi
//! refuses, shell commands, compaction, queued messages and thinking levels.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, Subscription, TestAppContext};
use pi_rpc::protocol::{ImageContent, ThinkingLevel};
use transcript::{BashItem, BashStatus, ItemKind, NoticeKind, RunStatus};

use super::session::{Lifecycle, PiSession, SessionEvent};
use crate::config::AppConfig;

/// A session on the fake pi, and what it emitted so far.
struct Fake {
    session: Entity<PiSession>,
    events: async_channel::Receiver<SessionEvent>,
    _subscription: Subscription,
}

impl Fake {
    fn start(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        cx.update(|cx| cx.set_global(AppConfig::for_tests(fake)));
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        Self {
            session,
            events,
            _subscription: subscription,
        }
    }

    async fn ready(cx: &mut TestAppContext) -> Self {
        let fake = Self::start(cx);
        fake.until(cx, |session| session.lifecycle() == &Lifecycle::Ready)
            .await;
        fake
    }

    /// Waits until the session is as `done` wants it.
    async fn until(&self, cx: &mut TestAppContext, done: impl Fn(&PiSession) -> bool) {
        while !cx.read(|cx| done(self.session.read(cx))) {
            self.events.recv().await.expect("session alive");
        }
    }

    async fn until_event(&self, wanted: impl Fn(&SessionEvent) -> bool) {
        while !wanted(&self.events.recv().await.expect("session alive")) {}
    }

    /// Waits for messages given back to the composer.
    async fn restored(&self) -> (String, Vec<ImageContent>) {
        loop {
            if let SessionEvent::RestoreToEditor {
                text,
                images,
                replace,
            } = self.events.recv().await.expect("session alive")
            {
                assert!(!replace, "a draft is kept");
                return (text, images);
            }
        }
    }

    fn submit(&self, text: &str, images: &[ImageContent], cx: &mut TestAppContext) {
        self.session.update(cx, |session, cx| {
            session.submit(text.into(), images.to_vec(), false, cx);
        });
    }

    fn run_bash(&self, command: &str, cx: &mut TestAppContext) {
        self.session.update(cx, |session, cx| {
            session.run_bash(command.into(), false, cx);
        });
    }

    fn compact(&self, instructions: &str, cx: &mut TestAppContext) {
        self.session.update(cx, |session, cx| {
            session.compact(Some(instructions.into()), cx);
        });
    }

    fn interrupt(&self, cx: &mut TestAppContext) {
        self.session.update(cx, |session, cx| session.interrupt(cx));
    }

    fn read<T>(&self, cx: &mut TestAppContext, read: impl Fn(&PiSession) -> T) -> T {
        cx.read(|cx| read(self.session.read(cx)))
    }

    fn is_busy(&self, cx: &mut TestAppContext) -> bool {
        self.read(cx, PiSession::is_busy)
    }

    /// The newest shell command.
    fn bash(&self, cx: &mut TestAppContext) -> BashItem {
        self.read(cx, |session| {
            let items = session.transcript().items();
            let bash = items.iter().rev().find_map(|item| match &item.kind {
                ItemKind::Bash(bash) => Some(bash.clone()),
                _ => None,
            });
            bash.expect("a shell command ran")
        })
    }

    fn prompts(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.read(cx, |session| {
            session
                .transcript()
                .items()
                .iter()
                .filter_map(|item| match &item.kind {
                    ItemKind::User(user) => Some(user.text.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    fn notices(&self, cx: &mut TestAppContext) -> Vec<(NoticeKind, String)> {
        self.read(cx, |session| {
            session
                .transcript()
                .items()
                .iter()
                .filter_map(|item| match &item.kind {
                    ItemKind::Notice(notice) => Some((notice.kind, notice.text.clone())),
                    _ => None,
                })
                .collect()
        })
    }
}

/// A file stays with its prompt, whose title and name are about what the user typed.
#[gpui_kit::test]
async fn a_file_is_part_of_its_prompt(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    let block = pi_sessions::prompt::block("paper.pdf", None, "## Page 1\n\ntext");
    let text = format!("{block}\n\nExplain this");
    fake.submit(&text, &[], cx);
    fake.until(cx, |session| {
        let mut items = session.transcript().items().iter();
        items.any(|item| matches!(item.kind, ItemKind::User(_)))
    })
    .await;
    let (typed, files, title) = fake.read(cx, |session| {
        let items = session.transcript().items();
        let ItemKind::User(user) = &items[0].kind else {
            panic!("not a prompt");
        };
        let names: Vec<_> = user.files.iter().map(|file| file.name.clone()).collect();
        (user.typed().to_owned(), names, session.display_title())
    });
    assert_eq!(fake.prompts(cx), [text]);
    assert_eq!(typed, "Explain this");
    assert_eq!(files, ["paper.pdf"]);
    assert_eq!(title.as_deref(), Some("Explain this"));
}

fn image(data: &str) -> ImageContent {
    ImageContent::new(data, "image/png")
}

/// A prompt pi refuses goes back to the composer with its images, and so do the ones behind
/// it, in order.
#[gpui_kit::test]
async fn a_refused_prompt_is_given_back(cx: &mut TestAppContext) {
    let fake = Fake::start(cx);
    fake.submit("refuse this", &[image("A")], cx);
    fake.submit("second", &[image("B")], cx);
    fake.submit("third", &[], cx);
    let (text, images) = fake.restored().await;
    assert_eq!(text, "refuse this\n\nsecond\n\nthird");
    assert_eq!(images, [image("A"), image("B")]);
    fake.until(cx, |session| !session.is_busy()).await;
    assert_eq!(fake.prompts(cx), [""; 0], "nothing else was sent");
    assert_eq!(
        fake.notices(cx),
        [(
            NoticeKind::Error,
            "No API key found for the selected model.".to_owned()
        )]
    );

    // The next one is sent as usual.
    fake.submit("again", &[], cx);
    fake.until(cx, |session| {
        !session.is_busy() && session.transcript().items().len() > 1
    })
    .await;
    assert_eq!(fake.prompts(cx), ["again"]);
}

/// pi stops before it answers a prompt: it goes back to the composer, before the ones that
/// waited behind it.
#[gpui_kit::test]
async fn a_prompt_is_given_back_when_pi_stops(cx: &mut TestAppContext) {
    let fake = Fake::start(cx);
    fake.submit("die", &[image("A")], cx);
    fake.submit("second", &[], cx);
    let (text, images) = fake.restored().await;
    assert_eq!(text, "die\n\nsecond");
    assert_eq!(images, [image("A")]);
    assert!(!fake.is_busy(cx));
}

/// Not one whose run had started: it is in the conversation.
#[gpui_kit::test]
async fn a_started_prompt_is_not_given_back(cx: &mut TestAppContext) {
    let fake = Fake::start(cx);
    fake.submit("start and die", &[], cx);
    fake.submit("second", &[], cx);
    let (text, _) = fake.restored().await;
    assert_eq!(text, "second");
}

/// A refused first prompt doesn't name the conversation; the first one pi accepts does.
#[gpui_kit::test]
async fn a_refused_first_prompt_does_not_name_the_conversation(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::AppSettings {
            auto_names: true,
            ..crate::settings::AppSettings::default()
        });
    });
    let fake = Fake::ready(cx).await;
    // Short enough to be the name as they are.
    fake.submit("refuse tests", &[], cx);
    fake.restored().await;
    fake.until(cx, |session| !session.is_busy()).await;
    assert_eq!(fake.read(cx, PiSession::title), None);

    fake.submit("fix tests", &[], cx);
    fake.until(cx, |session| session.title().is_some()).await;
    assert_eq!(
        fake.read(cx, PiSession::title).as_deref(),
        Some("fix tests")
    );
}

/// A shell command shows while it runs and keeps the chat busy; Esc stops it.
#[gpui_kit::test]
async fn a_shell_command_streams_and_stops(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    fake.run_bash("hang", cx);
    assert!(fake.is_busy(cx), "busy right away");
    assert!(matches!(fake.bash(cx).status, BashStatus::Running { .. }));
    assert!(fake.read(cx, PiSession::can_stop));
    assert!(!fake.read(cx, PiSession::is_running), "the agent rests");
    fake.until_event(|event| matches!(event, SessionEvent::BusyChanged))
        .await;
    fake.until(cx, |session| session.transcript().revision() > 1)
        .await;
    assert_eq!(fake.bash(cx).output, "working\n");

    fake.interrupt(cx);
    fake.until(cx, |session| !session.is_busy()).await;
    assert_eq!(fake.bash(cx).status, BashStatus::Cancelled);
    assert_eq!(fake.bash(cx).output, "working\n");
    let notices = fake.notices(cx);
    assert!(notices.is_empty(), "{notices:?}");
}

/// Every way a command ends leaves no running row behind.
#[gpui_kit::test]
async fn shell_commands_always_end(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    fake.run_bash("make", cx);
    fake.until(cx, |session| !session.is_busy()).await;
    let exit_code = Some(3);
    assert_eq!(fake.bash(cx).status, BashStatus::Done { exit_code });
    assert_eq!(fake.bash(cx).output, "part two");

    fake.run_bash("refuse", cx);
    fake.until(cx, |session| !session.is_busy()).await;
    assert_eq!(fake.bash(cx).status, BashStatus::Failed);
    assert_eq!(
        fake.notices(cx),
        [(NoticeKind::Error, "spawn failed".to_owned())]
    );

    fake.run_bash("die", cx);
    fake.until(cx, |session| {
        matches!(session.lifecycle(), Lifecycle::Exited { .. })
    })
    .await;
    assert_eq!(fake.bash(cx).status, BashStatus::Cancelled);
    assert!(!fake.read(cx, PiSession::is_bash_running));
    let notices = fake.notices(cx).len();
    // pi is gone: the command is not lost silently.
    fake.run_bash("make", cx);
    assert_eq!(fake.notices(cx).len(), notices + 1);
}

/// Like in pi's terminal, Esc stops the agent first and the shell command after that.
#[gpui_kit::test]
async fn esc_stops_the_agent_before_a_shell_command(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    fake.run_bash("hang", cx);
    fake.submit("hold", &[], cx);
    fake.until(cx, PiSession::is_running).await;
    fake.interrupt(cx);
    fake.until(cx, |session| !session.is_running()).await;
    assert!(fake.read(cx, PiSession::is_bash_running));
    fake.interrupt(cx);
    fake.until(cx, |session| !session.is_busy()).await;
    assert_eq!(fake.bash(cx).status, BashStatus::Cancelled);
}

/// `/compact` keeps the chat busy from the start, and prompts wait until it ends.
#[gpui_kit::test]
async fn prompts_wait_while_compacting(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    fake.compact("hang", cx);
    assert!(fake.is_busy(cx) && fake.read(cx, PiSession::shows_working));
    assert!(!fake.read(cx, PiSession::is_running), "nothing to steer");
    assert_eq!(
        fake.read(cx, PiSession::working_status),
        RunStatus::Compacting
    );
    fake.submit("first", &[], cx);
    fake.until(cx, |session| {
        *session.transcript().status() == RunStatus::Compacting
    })
    .await;
    fake.submit("second", &[], cx);
    assert_eq!(
        fake.read(cx, |session| session.pending_prompts().count()),
        2
    );

    fake.run_bash("finish compaction", cx);
    fake.until(cx, |session| {
        !session.is_busy() && session.transcript().items().len() > 6
    })
    .await;
    assert_eq!(fake.prompts(cx), ["first", "second"]);
    assert_eq!(
        fake.notices(cx),
        [(
            NoticeKind::Compaction,
            "Context compacted · 2.0k → ~1.0k tokens".to_owned()
        )]
    );
}

/// Esc cancels a compaction, gives back the prompts that waited for it, and one notice says
/// so.
#[gpui_kit::test]
async fn esc_cancels_a_compaction(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    fake.compact("hang", cx);
    fake.submit("wait", &[image("A")], cx);
    fake.interrupt(cx);
    let (text, images) = fake.restored().await;
    assert_eq!((text.as_str(), images), ("wait", vec![image("A")]));
    fake.until(cx, |session| !session.is_busy()).await;
    assert!(!fake.read(cx, PiSession::is_compacting));
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(
        fake.notices(cx),
        [(NoticeKind::Warning, "Compaction cancelled".to_owned())]
    );

    // A failed one is reported once as well.
    fake.compact("", cx);
    fake.until(cx, |session| !session.is_busy()).await;
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(
        fake.notices(cx)[1..],
        [(
            NoticeKind::Error,
            "Compaction failed: Nothing to compact".to_owned()
        )]
    );
}

/// Esc and Alt+Up give queued messages back with their images, also those pi expanded and
/// those without text, but not those already delivered.
#[gpui_kit::test]
async fn queued_images_are_given_back(cx: &mut TestAppContext) {
    let fake = Fake::ready(cx).await;
    fake.submit("hold", &[], cx);
    fake.until(cx, PiSession::is_running).await;
    let queued = async |count: usize, cx: &mut TestAppContext| {
        fake.until(cx, |session| {
            let queue = session.transcript().queue();
            queue.steering.len() == count && session.pending_prompts().count() == 0
        })
        .await;
        cx.run_until_parked();
    };
    fake.submit("delivered", &[image("D")], cx);
    queued(1, cx).await;
    fake.submit("", &[image("A")], cx);
    queued(2, cx).await;
    fake.submit("/skill:x", &[image("B")], cx);
    queued(3, cx).await;
    fake.run_bash("deliver", cx);
    queued(2, cx).await;
    fake.until(cx, |session| !session.is_bash_running()).await;

    fake.session.update(cx, |session, cx| session.dequeue(cx));
    let (text, images) = fake.restored().await;
    assert_eq!(text, "<skill>x</skill>");
    assert_eq!(images, [image("A"), image("B")]);
    assert!(fake.read(cx, PiSession::is_running), "not aborted");

    fake.submit("again", &[image("C")], cx);
    queued(1, cx).await;
    fake.interrupt(cx);
    let (text, images) = fake.restored().await;
    assert_eq!((text.as_str(), images), ("again", vec![image("C")]));
    fake.until(cx, |session| !session.is_busy()).await;
    assert_eq!(fake.prompts(cx), ["hold", "delivered"]);
}

/// Levels the model maps onto another one are not listed; a session on one of them shows that
/// other one, and Shift+Tab steps on from there.
#[gpui_kit::test]
async fn only_thinking_levels_that_differ_are_listed(cx: &mut TestAppContext) {
    use ThinkingLevel::{High, Low, Medium, Minimal, Off, Xhigh};

    let fake = Fake::ready(cx).await;
    let levels = |cx: &mut TestAppContext| {
        fake.read(cx, |session| {
            (session.thinking_levels(), session.thinking_level())
        })
    };
    assert_eq!(levels(cx), (vec![Off, Low, Medium, Xhigh], Some(Low)));
    let set = |level, cx: &mut TestAppContext| {
        fake.session
            .update(cx, |session, cx| session.set_thinking_level(level, cx));
        levels(cx).1
    };
    assert_eq!(set(Minimal, cx), Some(Low));
    assert_eq!(set(High, cx), Some(Xhigh));
    fake.session
        .update(cx, |session, cx| session.cycle_thinking_level(cx));
    assert_eq!(levels(cx).1, Some(Off));
}
