//! Settings → Sandboxing & Security: the sandbox the agent's tools run in (local Code sessions), and which
//! project files pi loads (its project trust). The state is [`crate::security::Security`].

use std::path::Path;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sandbox::{Backend, Network};

use crate::security::{Security, Test, TrustDefault, security, tilde};
use crate::theme::palette;
use crate::ui::AppIcon;

/// The issue about a sandbox on Windows.
const WINDOWS_ISSUE: &str = "https://github.com/seba2390/pistachio/issues/47";

pub(super) fn page() -> SettingPage {
    SettingPage::new("Sandboxing & Security")
        .icon(AppIcon::ShieldCheck)
        .description(
            "What the agent can do on this computer in Code sessions. Sessions on SSH hosts run \
             there, and chats have no tools.",
        )
        .group(
            SettingGroup::new()
                .title("Sandbox")
                .item(
                    SettingItem::render(|_, _, cx| status(cx))
                        .keywords(["sandbox", "Seatbelt", "bubblewrap", "Landlock"]),
                )
                .item(
                    SettingItem::new(
                        "Sandbox Code sessions",
                        super::view::hover_switch(
                            "sandbox-enabled",
                            |cx| read(cx, |s| s.settings.enabled),
                            |on, cx| change(cx, |settings| settings.enabled = on),
                        ),
                    )
                    .description(
                        "The agent's commands run in a sandbox, and its file tools keep to the same \
                         rules. Changes apply from the next command, also in running sessions. \
                         pi itself, its logins and model requests, MCP servers and the commands you \
                         run yourself with ! are not sandboxed.",
                    ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Files")
                .description(
                    "Your home folder is hidden from the agent, except the session's folder and \
                     what you allow here. Programs and system files outside it stay readable, \
                     since commands need them. Changes are allowed only in the session's \
                     folder, temporary files and the git folder its commits need, plus what you \
                     allow to change.",
                )
                .item(
                    SettingItem::new(
                        "Let tools use their folders",
                        super::view::hover_switch(
                            "sandbox-tool-folders",
                            |cx| read(cx, |s| s.settings.tool_folders),
                            |on, cx| change(cx, |settings| settings.tool_folders = on),
                        ),
                    )
                    .description(
                        "~/.cargo, ~/.npm, ~/.nvm, ~/.gitconfig and the other folders in your home \
                         folder where tools keep their programs, settings and downloads, so \
                         builds and installs work. pi's skills can always be read.",
                    ),
                )
                .item(SettingItem::render(|_, _, cx| allowed(cx)).keywords([
                    "allow", "folder", "read", "write", "hidden", "home",
                ])),
        )
        .group(
            SettingGroup::new().title("Network").item(
                SettingItem::new(
                    "Commands may use the network",
                    SettingField::dropdown(
                        vec![
                            ("on".into(), "Yes".into()),
                            ("off".into(), "No".into()),
                        ],
                        |cx| {
                            match read(cx, |s| s.settings.network) {
                                Network::On => "on",
                                Network::Off => "off",
                            }
                            .into()
                        },
                        |value, cx| {
                            let network = if value.as_ref() == "off" {
                                Network::Off
                            } else {
                                Network::On
                            };
                            change(cx, |settings| settings.network = network);
                        },
                    ),
                )
                .description(network_description()),
            ),
        )
        .group(
            SettingGroup::new()
                .title("Projects without a sandbox")
                .description(
                    "Their commands run as you, for example to build an app with Xcode. Turn \
                     the sandbox off for a project from the badge at the top of its session.",
                )
                .item(SettingItem::render(|_, _, cx| unsandboxed(cx))),
        )
        .group(
            SettingGroup::new()
                .title("Test")
                .item(SettingItem::render(|_, _, cx| test(cx)).keywords(["test", "check"])),
        )
        .group(
            SettingGroup::new()
                .title("Project files")
                .description(
                    "A project's .pi folder can bring its own extensions (code that runs inside \
                     pi), MCP servers, skills and settings. pi loads them only for projects you \
                     trust (on the Skills page, or /trust in the terminal). AGENTS.md and \
                     CLAUDE.md are always read. Changes apply to sessions started afterwards.",
                )
                .item(
                    SettingItem::new(
                        "Without a decision",
                        SettingField::dropdown(
                            TrustDefault::ALL
                                .into_iter()
                                .map(|value| (value.id().into(), value.label().into()))
                                .collect(),
                            |cx| read(cx, |s| s.trust_default).id().into(),
                            |value, cx| {
                                let value = TrustDefault::from_id(value.as_ref());
                                security(cx).update(cx, |s, cx| s.set_trust_default(value, cx));
                            },
                        ),
                    )
                    .description("pi's defaultProjectTrust setting.")
                    .keywords(["trust", "project", "defaultProjectTrust", "extensions"]),
                )
                .item(SettingItem::render(|_, _, cx| trusted(cx)).keywords(["trusted"])),
        )
}

fn network_description() -> &'static str {
    "With No, commands can't connect to the Internet: installs and downloads fail. Model \
     requests are pi's own and always work, and your API keys never reach commands: keys in \
     environment variables (names ending in _API_KEY, _TOKEN, …) are removed."
}

/// Reads the state (made when the Settings page opened).
fn read<T: Default>(cx: &App, f: impl FnOnce(&Security) -> T) -> T {
    crate::security::existing(cx).map_or_else(T::default, |state| f(state.read(cx)))
}

fn change(cx: &mut App, f: impl FnOnce(&mut sandbox::Settings)) {
    security(cx).update(cx, |s, cx| s.update(cx, f));
}

/// Which sandbox this computer has, and what to do when it has none.
fn status(cx: &mut App) -> AnyElement {
    let state = security(cx).read(cx);
    let theme = cx.theme();
    let (warning, success) = (palette(cx).warning, palette(cx).success);
    let enabled = state.settings.enabled;
    let (line, color, problem, fix) = match &state.status {
        None => (
            "Checking the sandbox…".to_owned(),
            theme.muted_foreground,
            None,
            None,
        ),
        Some(status) => match &status.backend {
            Some(backend) if enabled => (
                format!("Commands run in a sandbox ({}).", backend.label()),
                success,
                match backend {
                    // Why not bubblewrap.
                    Backend::Landlock => status.problem.clone(),
                    _ => None,
                },
                status.fix.clone(),
            ),
            Some(backend) => (
                format!("The sandbox is off. This computer has {}.", backend.label()),
                warning,
                None,
                None,
            ),
            None => (
                "Commands are not sandboxed on this computer.".to_owned(),
                warning,
                status
                    .problem
                    .clone()
                    .map(|problem| capitalize(&problem) + "."),
                status.fix.clone(),
            ),
        },
    };
    let windows = !sandbox::SUPPORTED;
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color)
                        .debug_selector(|| "sandbox-status".into())
                        .child(line),
                )
                .when(!windows, |this| {
                    this.child(
                        Button::new("sandbox-check-again")
                            .label("Check again")
                            .xsmall()
                            .ghost()
                            .disabled(state.status.is_none())
                            .on_click(|_, _, cx| security(cx).update(cx, Security::check)),
                    )
                }),
        )
        .when_some(problem, |this, problem| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(problem),
            )
        })
        .when_some(fix, |this, fix| {
            this.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("To use bubblewrap:")
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.foreground)
                            .child(fix.clone()),
                    )
                    .child(
                        Button::new("sandbox-copy-fix")
                            .label("Copy")
                            .xsmall()
                            .ghost()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(fix.clone()));
                            }),
                    ),
            )
        })
        .when(windows, |this| {
            this.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Commands run as you. A sandbox for Windows is planned.")
                    .child(
                        Button::new("sandbox-windows-issue")
                            .label("Follow it on GitHub")
                            .xsmall()
                            .ghost()
                            .on_click(|_, _, cx| cx.open_url(WINDOWS_ISSUE)),
                    ),
            )
        })
        .into_any_element()
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A list of paths, each with Remove, and buttons below it.
#[allow(clippy::too_many_arguments)] // one list's texts and actions
fn path_list(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    paths: Vec<String>,
    empty: &'static str,
    remove: impl Fn(usize, &mut App) + 'static,
    buttons: Vec<Button>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let remove = std::rc::Rc::new(remove);
    let rows = paths.into_iter().enumerate().map(|(index, path)| {
        let remove = remove.clone();
        h_flex()
            .w_full()
            .gap_2()
            .py_0p5()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .child(path.clone()),
            )
            .child(
                Button::new(SharedString::from(format!("{id}-remove-{index}")))
                    .label("Remove")
                    .accessibility_label(format!("Remove {path}"))
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| remove(index, cx)),
            )
    });
    let rows: Vec<_> = rows.collect();
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .debug_selector(move || id.into())
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(description),
        )
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .child(empty),
            )
        })
        .children(rows)
        .child(h_flex().gap_2().pt_1().children(buttons))
        .into_any_element()
}

/// Asks for folders (or files) and passes them on.
fn pick(
    files: bool,
    prompt: &'static str,
    cx: &mut App,
    chosen: impl FnOnce(Vec<String>, &mut App) + 'static,
) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files,
        directories: true,
        multiple: true,
        prompt: Some(prompt.into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = paths.await {
            let paths = paths.iter().map(|path| tilde(path)).collect();
            cx.update(|cx| chosen(paths, cx));
        }
    })
    .detach();
}

/// The folders outside the project the agent may use, each read-only or also to change.
fn allowed(cx: &mut App) -> AnyElement {
    let entries = read(cx, |s| s.settings.allowed.clone());
    let theme = cx.theme();
    let rows: Vec<_> = entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let access = |write: bool, label: &'static str| {
                Button::new(SharedString::from(format!(
                    "sandbox-allowed-{index}-{}",
                    if write { "write" } else { "read" }
                )))
                .label(label)
                .xsmall()
                .ghost()
                .toggled(entry.write == write)
                .on_click(move |_, _, cx| {
                    change(cx, |settings| {
                        if let Some(entry) = settings.allowed.get_mut(index) {
                            entry.write = write;
                        }
                    });
                })
            };
            h_flex()
                .w_full()
                .gap_2()
                .py_0p5()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .child(entry.path.clone()),
                )
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(access(false, "Read"))
                        .child(access(true, "Read and change")),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "sandbox-allowed-remove-{index}"
                    )))
                    .label("Remove")
                    .accessibility_label(format!("Remove {}", entry.path))
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        change(cx, |settings| {
                            if index < settings.allowed.len() {
                                settings.allowed.remove(index);
                            }
                        });
                    }),
                )
        })
        .collect();
    let add = Button::new("sandbox-add-allowed")
        .label("Add folder…")
        .xsmall()
        .outline()
        .on_click(|_, _, cx| {
            pick(false, "Allow", cx, |paths, cx| {
                change(cx, |settings| {
                    for path in paths {
                        crate::security::allow(settings, sandbox::Allowed { path, write: false });
                    }
                });
            });
        });
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .debug_selector(|| "sandbox-allowed".into())
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child("Allowed folders"),
        )
        .child(div().text_xs().text_color(theme.muted_foreground).child(
            "Folders outside the project the agent may read, or also change. Keys and \
                     logins (~/.ssh, ~/.aws, ~/.gnupg, pi's own and others) stay hidden even \
                     inside an allowed folder.",
        ))
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .child("None yet."),
            )
        })
        .children(rows)
        .child(h_flex().pt_1().child(add))
        .into_any_element()
}

fn unsandboxed(cx: &mut App) -> AnyElement {
    let paths = read(cx, |s| s.settings.unsandboxed.clone());
    let add = Button::new("sandbox-add-unsandboxed")
        .label("Add project…")
        .xsmall()
        .outline()
        .on_click(|_, _, cx| {
            pick(false, "Run without a sandbox", cx, |paths, cx| {
                change(cx, |settings| {
                    for path in paths {
                        if !settings.unsandboxed.contains(&path) {
                            settings.unsandboxed.push(path);
                        }
                    }
                });
            });
        });
    path_list(
        "sandbox-unsandboxed",
        "Projects",
        "Also their subfolders.",
        paths,
        "None: every project is sandboxed.",
        |index, cx| {
            change(cx, |settings| {
                if index < settings.unsandboxed.len() {
                    settings.unsandboxed.remove(index);
                }
            });
        },
        vec![add],
        cx,
    )
}

/// The self-test: a button and what it found.
fn test(cx: &mut App) -> AnyElement {
    let state = security(cx).read(cx);
    let theme = cx.theme();
    let (success, warning) = (palette(cx).success, palette(cx).warning);
    let running = state.test == Test::Running;
    let available = state
        .status
        .as_ref()
        .is_some_and(|status| status.backend.is_some());
    let results: Vec<AnyElement> = match &state.test {
        Test::NotRun => Vec::new(),
        Test::Running => vec![
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Testing…")
                .into_any_element(),
        ],
        Test::Failed(error) => vec![
            div()
                .text_xs()
                .text_color(warning)
                .child(format!("The test could not run: {error}"))
                .into_any_element(),
        ],
        Test::Done(report) if report.checks.is_empty() => vec![
            div()
                .text_xs()
                .text_color(warning)
                .child("There is no sandbox to test on this computer.")
                .into_any_element(),
        ],
        Test::Done(report) => report
            .checks
            .iter()
            .map(|check| {
                let (mark, color) = match check.passed {
                    Some(true) => ("✓", success),
                    Some(false) => ("✗", warning),
                    None => ("–", theme.muted_foreground),
                };
                h_flex()
                    .w_full()
                    .gap_2()
                    .text_xs()
                    .child(div().w(px(12.)).text_color(color).child(mark))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(check.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.muted_foreground)
                            .child(check.detail.clone()),
                    )
                    .into_any_element()
            })
            .collect(),
    };
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .debug_selector(|| "sandbox-test".into())
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            "Runs a few commands in a scratch folder with the settings above: \
                             a change inside it, one outside, reading a hidden path and, when it \
                             is off, the network.",
                        ),
                )
                .child(
                    Button::new("sandbox-run-test")
                        .label("Test the sandbox")
                        .xsmall()
                        .outline()
                        .disabled(running || !available)
                        .on_click(|_, _, cx| security(cx).update(cx, Security::run_test)),
                ),
        )
        .children(results)
        .into_any_element()
}

/// The saved trust decisions, each with Remove.
fn trusted(cx: &mut App) -> AnyElement {
    let (rows, error) = read(cx, |s| (s.trusted.clone(), s.error.clone()));
    let theme = cx.theme();
    let warning = palette(cx).warning;
    let list = rows
        .into_iter()
        .enumerate()
        .map(|(index, (folder, trusted))| {
            let forget = folder.clone();
            h_flex()
                .w_full()
                .gap_2()
                .py_0p5()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .child(tilde(Path::new(&folder))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if trusted { "Trusted" } else { "Not trusted" }),
                )
                .child(
                    Button::new(SharedString::from(format!("trust-forget-{index}")))
                        .label("Remove")
                        .accessibility_label(format!("Remove the decision for {folder}"))
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            let folder = forget.clone();
                            security(cx).update(cx, |s, cx| s.forget_trust(&folder, cx));
                        }),
                )
        });
    let list: Vec<_> = list.collect();
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .debug_selector(|| "trust-decisions".into())
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child("Decisions"),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Saved in pi's trust.json; a decision covers the folder's subfolders."),
        )
        .when(list.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .child("None yet."),
            )
        })
        .children(list)
        .when_some(error, |this, error| {
            this.child(div().text_xs().text_color(warning).child(error))
        })
        .into_any_element()
}
