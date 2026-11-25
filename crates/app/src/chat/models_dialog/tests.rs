use std::collections::HashMap;

use gpui_kit::ParentElement as _;
use gpui_kit::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext};
use pi_config::ModelsFile;
use pretty_assertions::assert_eq;
use remote::HostId;

use super::probe::tests::{ollama, serve};
use super::{ModelsDialog, Open, Place, Screen, count, grouped};
use crate::config::AppConfig;
use crate::hosts::{Hosts, HostsHandle};
use crate::pi_place::PiPlace;
use crate::ui::dialog::open_form_dialog;
use crate::ui::dialog::testing::{click, draw, is_open, press, window};

/// The dialog on a models.json in `dir` that says `text`.
fn open<'a>(
    dir: &std::path::Path,
    text: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<ModelsDialog>, &'a mut VisualTestContext) {
    // Servers are really asked.
    cx.executor().allow_parking();
    let cx = window(cx);
    let place = local(dir);
    if !text.is_empty() {
        place.write(text).unwrap();
    }
    let file = ModelsFile::parse(text).unwrap();
    let dialog = cx.update(|window, cx| {
        cx.set_global(AppConfig::for_tests("pi".into()));
        let hosts = cx.new(|_| Hosts::for_tests(Vec::new()));
        cx.set_global(HostsHandle(hosts));
        // A host that is not connected: pi's own files on this computer are not read.
        let host = Some(HostId("none".into()));
        let dialog = cx.new(|cx| {
            let mut dialog =
                ModelsDialog::new(host, Open::Start, HashMap::new(), false, window, cx);
            dialog.place = Some(place);
            dialog.file_loaded(file, window, cx);
            dialog
        });
        let view = dialog.clone();
        open_form_dialog(None, window, cx, move |shell, _, _| {
            shell.child(view.clone())
        });
        dialog
    });
    draw(cx);
    (dialog, cx)
}

fn local(agent_dir: &std::path::Path) -> Place {
    Place {
        pi: PiPlace::Local {
            agent_dir: agent_dir.into(),
        },
        path: agent_dir.join("models.json").to_string_lossy().into_owned(),
    }
}

fn saved(dir: &std::path::Path) -> ModelsFile {
    ModelsFile::parse(&std::fs::read_to_string(dir.join("models.json")).unwrap()).unwrap()
}

/// Waits for what is asked of servers, on real threads.
fn settle(dialog: &Entity<ModelsDialog>, cx: &mut VisualTestContext) {
    let task = cx.update(|_, cx| dialog.update(cx, |dialog, _| dialog.task.take()));
    if let Some(task) = task {
        cx.foreground_executor().clone().block_test(task);
    }
    draw(cx);
}

fn connect(dialog: &Entity<ModelsDialog>, address: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let focus = dialog.read(cx).address.focus_handle(cx);
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_input(address);
    press("enter", cx);
    settle(dialog, cx);
}

/// The address is all that is typed; the one model for conversations is ticked already.
#[gpui_kit::test]
fn adds_a_server_by_its_address(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let address = serve(ollama);
    let (dialog, cx) = open(dir.path(), "", cx);
    connect(&dialog, &address, cx);
    click("save-server", cx);
    let providers = saved(dir.path()).providers();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "ollama");
    assert_eq!(
        providers[0].base_url.as_deref(),
        Some(format!("http://{address}/v1").as_str())
    );
    assert_eq!(providers[0].api.as_deref(), Some("openai-completions"));
    assert_eq!(providers[0].api_key.as_deref(), Some("none"));
    let [model] = providers[0].models.as_slice() else {
        panic!("{providers:?}");
    };
    assert_eq!(model.spec.id, "qwen3:8b");
    assert_eq!(model.spec.context_window, Some(40_960));
    assert_eq!(model.spec.max_tokens, Some(10_240), "fits the context");
    assert!(model.spec.reasoning);
}

/// Nothing typed is lost when there is no server, and Enter does not close the dialog.
#[gpui_kit::test]
fn an_address_that_does_not_answer(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (dialog, cx) = open(dir.path(), "", cx);
    connect(&dialog, "not an address", cx);
    assert!(is_open(cx));
    cx.update(|_, cx| assert!(dialog.read(cx).address_error.is_some()));

    // Models can be added by their IDs all the same.
    cx.update(|window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog
                .address
                .update(cx, |input, cx| input.set_value("", window, cx));
        });
    });
    connect(&dialog, "127.0.0.1:9", cx);
    assert!(cx.debug_bounds("retry").is_some());
    cx.update(|window, cx| {
        let Screen::Server(screen) = &dialog.read(cx).screen else {
            panic!("no server");
        };
        let focus = screen.by_id.focus_handle(cx);
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_input("my-model");
    press("enter", cx);
    assert!(is_open(cx));
    click("save-server", cx);
    let provider = &saved(dir.path()).providers()[0];
    assert_eq!(provider.base_url.as_deref(), Some("http://127.0.0.1:9/v1"));
    assert_eq!(provider.models[0].spec.id, "my-model");
}

#[gpui_kit::test]
fn a_server_that_wants_a_key(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let address =
        serve(
            |_, path, headers, _| match (path, headers.contains("authorization: bearer good")) {
                ("/v1/models", true) => (200, r#"{"data":[{"id":"m"}]}"#.into()),
                _ => (401, "{}".into()),
            },
        );
    let (dialog, cx) = open(dir.path(), "", cx);
    connect(&dialog, &address, cx);
    // The field has the focus.
    cx.simulate_input("bad");
    click("use-key", cx);
    settle(&dialog, cx);
    assert!(cx.debug_bounds("use-key").is_some());
    cx.simulate_input("good");
    cx.update(|window, cx| {
        dialog.update(cx, |dialog, cx| {
            let Screen::Server(screen) = &dialog.screen else {
                panic!("no server");
            };
            screen
                .key
                .update(cx, |key, cx| key.set_value("good", window, cx));
        });
    });
    click("use-key", cx);
    settle(&dialog, cx);
    assert!(cx.debug_bounds("use-key").is_none());
    click("save-server", cx);
    assert_eq!(
        saved(dir.path()).providers()[0].api_key.as_deref(),
        Some("good")
    );
}

/// What the form does not know of a model stays; a model that is not ticked any more goes.
#[gpui_kit::test]
fn changes_what_is_connected(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"providers":{"spark":{"baseUrl":"http://127.0.0.1:9/v1","api":"openai-completions",
        "apiKey":"k","compat":{"thinkingFormat":"openai"},"models":[
        {"id":"a","thinkingLevelMap":{"high":"xhigh"}},{"id":"b"}]}}}"#;
    let (dialog, cx) = open(dir.path(), text, cx);
    click("provider:spark", cx);
    settle(&dialog, cx);
    click("tick:b", cx);
    click("details:a", cx);
    cx.update(|window, cx| {
        let Screen::Server(screen) = &dialog.read(cx).screen else {
            panic!("no server");
        };
        let fields = screen.rows[0].fields.as_ref().unwrap();
        let focus = fields.context_window.focus_handle(cx);
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_input("x");
    click("save-server", cx);
    cx.update(|_, cx| {
        let Screen::Server(screen) = &dialog.read(cx).screen else {
            panic!("no server");
        };
        assert_eq!(
            screen.error.as_deref(),
            Some("a: The context window must be a number of tokens.")
        );
    });
    press("backspace", cx);
    cx.simulate_input("65,536");
    press("enter", cx);
    let file = saved(dir.path());
    let provider = &file.providers()[0];
    assert_eq!(provider.models.len(), 1);
    assert_eq!(provider.models[0].spec.context_window, Some(65_536));
    let text = file.to_text();
    assert!(text.contains("thinkingLevelMap") && text.contains("thinkingFormat"));
    assert_eq!(provider.api_key.as_deref(), Some("k"));
}

#[gpui_kit::test]
fn removing_asks_first(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let text =
        r#"{"providers":{"spark":{"baseUrl":"http://127.0.0.1:9/v1","models":[{"id":"a"}]}}}"#;
    let (dialog, cx) = open(dir.path(), text, cx);
    click("provider:spark", cx);
    settle(&dialog, cx);
    click("connection-options", cx);
    click("remove-server", cx);
    assert_eq!(saved(dir.path()).providers().len(), 1);
    click("remove-server", cx);
    assert_eq!(saved(dir.path()).providers(), Vec::new());
}

/// A key that is looked up by pi is not checked; an empty one is not saved.
#[gpui_kit::test]
fn a_provider_needs_only_a_key(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (dialog, cx) = open(dir.path(), "", cx);
    click("built-in:openrouter", cx);
    press("enter", cx);
    assert!(is_open(cx));
    assert!(!dir.path().join("models.json").exists());
    cx.simulate_input("$MY_KEY");
    press("enter", cx);
    settle(&dialog, cx);
    let file = saved(dir.path());
    assert_eq!(file.providers()[0].api_key.as_deref(), Some("$MY_KEY"));
    assert_eq!(file.providers()[0].built_in().unwrap().id, "openrouter");
}

#[gpui_kit::test]
fn removes_a_key(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"providers":{"openrouter":{"apiKey":"k"}}}"#;
    let (_, cx) = open(dir.path(), text, cx);
    click("provider:openrouter", cx);
    click("remove-key", cx);
    assert_eq!(saved(dir.path()).providers(), Vec::new());
}

#[gpui_kit::test]
fn enter_is_a_newline_in_the_json(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (dialog, cx) = open(dir.path(), "", cx);
    click("edit-models-json", cx);
    cx.update(|window, cx| {
        let focus = dialog.read(cx).editor.focus_handle(cx);
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_input("a");
    press("enter", cx);
    cx.simulate_input("b");
    assert!(is_open(cx));
    let text = cx.update(|_, cx| dialog.read(cx).editor.read(cx).value().to_string());
    assert_eq!(text, "a\nb");
    click("models-back", cx);
    assert!(cx.debug_bounds("connect").is_some());
}

#[test]
fn local_file_round_trip_keeps_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let place = local(&dir.path().join("agent"));
    assert_eq!(place.read().unwrap(), "");
    place.write("{\"a\": 1}\n").unwrap();
    place.write("{\"b\": 2}\n").unwrap();
    assert_eq!(place.read().unwrap(), "{\"b\": 2}\n");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("agent/models.json.bak-pistachio")).unwrap(),
        "{\"a\": 1}\n"
    );
}

#[test]
fn numbers_for_people() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(1_048_576), "1,048,576");
    assert_eq!(count(1, "model"), "1 model");
    assert_eq!(count(1234, "model"), "1,234 models");
}

#[test]
fn keys_to_ask_a_server_with() {
    let place = local(&std::env::temp_dir());
    assert_eq!(place.key(" sk ").as_deref(), Some("sk"));
    assert_eq!(place.key("!pass x"), None);
    assert_eq!(place.key("$PISTACHIO_NO_SUCH_VARIABLE"), None);
    assert_eq!(place.key("${HOME}"), std::env::var("HOME").ok());
}
