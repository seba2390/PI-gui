//! Drives a real [`PiSession`] against the scripted fake pi from `pi-rpc`'s tests: prompts pi
//! refuses, shell commands, compaction and queued messages.

use std::path::PathBuf;

use gpui_kit::{AppContext as _, Entity, Subscription, TestAppContext};
use pi_rpc::protocol::ImageContent;
use transcript::{ItemKind, NoticeKind};

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

    fn read<T>(&self, cx: &mut TestAppContext, read: impl Fn(&PiSession) -> T) -> T {
        cx.read(|cx| read(self.session.read(cx)))
    }

    fn is_busy(&self, cx: &mut TestAppContext) -> bool {
        self.read(cx, PiSession::is_busy)
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
