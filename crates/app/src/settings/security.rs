//! Settings → Sandboxing & Security: the sandbox the agent's tools run in (local Code sessions),
//! what it may use (folders, the network) and which project files pi loads (its project trust).
//! The state is [`crate::security::Security`].

use std::path::Path;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sandbox::{Backend, Network};

use crate::security::{Security, TrustDefault, security, tilde};
use crate::theme::palette;
use crate::ui::AppIcon;

/// The issue about a sandbox on Windows.
const WINDOWS_ISSUE: &str = "https://github.com/seba2390/pistachio/issues/47";
/// How tall a list gets before it scrolls.
const LIST_MAX_HEIGHT: f32 = 232.;

/// `site_input` is the allowlist's field (the view keeps it, with its text).
pub(super) fn page(site_input: Entity<InputState>) -> SettingPage {
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
                .item(SettingItem::render(|_, _, cx| allowed_folders(cx)).keywords([
                    "allow", "folder", "read", "write", "hidden", "home",
                ])),
        )
        .group(
            SettingGroup::new()
                .title("Network")
                .item(
                    SettingItem::render(|_, _, cx| network_choice(cx))
                        .keywords(["network", "internet", "sites", "proxy"]),
                )
                .item(
                    SettingItem::render(move |_, window, cx| sites(&site_input, window, cx))
                        .keywords(["allowlist", "sites", "domains", "url"]),
                ),
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

/// Reads the state (made when the Settings page opened).
fn read<T: Default>(cx: &App, f: impl FnOnce(&Security) -> T) -> T {
    crate::security::existing(cx).map_or_else(T::default, |state| f(state.read(cx)))
}

fn change(cx: &mut App, f: impl FnOnce(&mut sandbox::Settings)) {
    security(cx).update(cx, |s, cx| s.update(cx, f));
}

/// A titled box holding a list (scrolling when long) and, under it, how to add to it.
fn list_card(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    rows: Vec<AnyElement>,
    empty: &'static str,
    footer: AnyElement,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let count = rows.len();
    let rows = rows.into_iter().enumerate().map(|(index, row)| {
        div()
            .w_full()
            .px_3()
            .py_1p5()
            .when(index + 1 < count, |this| {
                this.border_b_1().border_color(theme.border.opacity(0.6))
            })
            .child(row)
    });
    v_flex()
        .w_full()
        .gap_1p5()
        .py_1()
        .debug_selector(move || id.into())
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(description),
        )
        .child(
            v_flex()
                .w_full()
                .mt_1()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background.opacity(0.6))
                .overflow_hidden()
                .child(
                    div()
                        .id(id)
                        .w_full()
                        .max_h(px(LIST_MAX_HEIGHT))
                        .overflow_y_scroll()
                        .when(count == 0, |this| {
                            this.child(
                                h_flex()
                                    .w_full()
                                    .justify_center()
                                    .py_4()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(empty),
                            )
                        })
                        .children(rows),
                )
                .child(
                    div()
                        .w_full()
                        .border_t_1()
                        .border_color(theme.border)
                        .px_3()
                        .py_2()
                        .child(footer),
                ),
        )
        .into_any_element()
}

/// The round "+" in the middle of a list's footer.
fn add_button(id: &'static str, tooltip: &'static str) -> Button {
    Button::new(id)
        .icon(IconName::Plus)
        .small()
        .outline()
        .rounded(px(999.))
        .tooltip(tooltip)
        .accessibility_label(tooltip)
}

/// A small icon button that removes a row.
fn remove_button(id: SharedString, what: String) -> Button {
    Button::new(id)
        .icon(Icon::from(AppIcon::Trash))
        .xsmall()
        .ghost()
        .tooltip("Remove")
        .accessibility_label(format!("Remove {what}"))
}

/// The folders outside the project the agent may use, each to read or also to change.
fn allowed_folders(cx: &mut App) -> AnyElement {
    let entries = read(cx, |s| s.settings.allowed.clone());
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
    let rows = entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let access = |write: bool| {
                let (icon, label) = if write {
                    (Icon::from(AppIcon::Pencil), "Read and change")
                } else {
                    (Icon::new(IconName::Eye), "Read")
                };
                Button::new(SharedString::from(format!(
                    "allowed-{index}-{}",
                    if write { "write" } else { "read" }
                )))
                .icon(icon)
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
                .child(Icon::new(IconName::Folder).small().text_color(muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(mono.clone())
                        .child(entry.path.clone()),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_0p5()
                        .child(access(false))
                        .child(access(true)),
                )
                .child(
                    remove_button(
                        SharedString::from(format!("allowed-remove-{index}")),
                        entry.path.clone(),
                    )
                    .on_click(move |_, _, cx| {
                        change(cx, |settings| {
                            if index < settings.allowed.len() {
                                settings.allowed.remove(index);
                            }
                        });
                    }),
                )
                .into_any_element()
        })
        .collect();
    let add = add_button("allowed-add", "Add a folder").on_click(|_, _, cx| {
        pick(false, "Allow", cx, |paths, cx| {
            change(cx, |settings| {
                for path in paths {
                    crate::security::allow(settings, sandbox::Allowed { path, write: false });
                }
            });
        });
    });
    list_card(
        "sandbox-allowed",
        "Allowed folders",
        "Folders outside the project the agent may read, or also change. Keys and logins \
         (~/.ssh, ~/.aws, ~/.gnupg, pi's own and others) stay hidden even inside an allowed \
         folder.",
        rows,
        "No folders yet",
        h_flex()
            .w_full()
            .justify_center()
            .child(add)
            .into_any_element(),
        cx,
    )
}

/// None, All or Custom, side by side.
fn network_choice(cx: &mut App) -> AnyElement {
    let (network, blockable) = read(cx, |s| {
        (
            s.settings.network,
            s.status.as_ref().is_none_or(|status| status.network),
        )
    });
    let theme = cx.theme();
    let warning = palette(cx).warning;
    let option = |value: Network, icon: Icon, label: &'static str| {
        Button::new(SharedString::from(format!("network-{label}")))
            .icon(icon)
            .label(label)
            .small()
            .ghost()
            .toggled(network == value)
            .on_click(move |_, _, cx| change(cx, |settings| settings.network = value))
    };
    let description = match network {
        Network::Off => "Commands can't connect to the Internet: installs and downloads fail.",
        Network::On => "Commands can connect to any site.",
        Network::Custom => {
            "Commands reach only the sites below, through a proxy in Pistachio. Tools that \
             don't use a proxy (most do: curl, git, npm, pip, cargo) get no network."
        }
    };
    v_flex()
        .w_full()
        .gap_1p5()
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
                        .child("Commands may reach"),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_0p5()
                        .p_0p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .child(option(Network::Off, Icon::from(AppIcon::GlobeOff), "None"))
                        .child(option(Network::On, Icon::from(AppIcon::Globe), "All"))
                        .child(option(
                            Network::Custom,
                            Icon::from(AppIcon::GlobeLock),
                            "Custom",
                        )),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(description),
        )
        .when(network != Network::On && !blockable, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(warning)
                    .child("This system can't limit the network (Landlock needs Linux 6.7)."),
            )
        })
        .child(div().text_xs().text_color(theme.muted_foreground).child(
            "Model requests are pi's own and always work. Your API keys never reach \
                     commands: keys in environment variables (names ending in _API_KEY, _TOKEN, \
                     …) are removed.",
        ))
        .into_any_element()
}

/// With a limited network: the allowed sites, and a field that checks an address as it is typed.
fn sites(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> AnyElement {
    let (network, sites, check) = read(cx, |s| {
        (
            s.settings.network,
            s.settings.sites.clone(),
            s.site_check.clone(),
        )
    });
    if network != Network::Custom {
        return div().into_any_element();
    }
    let theme = cx.theme();
    let (muted, mono, border) = (
        theme.muted_foreground,
        theme.mono_font_family.clone(),
        theme.border,
    );
    let (success, warning, error) = (palette(cx).success, palette(cx).warning, palette(cx).error);
    let rows = sites
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let (host, subdomains) = match label.strip_suffix("/*") {
                Some(host) => (host.to_owned(), true),
                None => (label.clone(), false),
            };
            h_flex()
                .w_full()
                .gap_2()
                .child(Icon::new(IconName::Globe).small().text_color(muted))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(mono.clone())
                        .child(host),
                )
                .when(subdomains, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .px_1p5()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .text_xs()
                            .text_color(muted)
                            .child("and subdomains"),
                    )
                })
                .child(div().flex_1())
                .child(
                    remove_button(
                        SharedString::from(format!("site-remove-{index}")),
                        label.clone(),
                    )
                    .on_click(move |_, _, cx| {
                        change(cx, |settings| {
                            if index < settings.sites.len() {
                                settings.sites.remove(index);
                            }
                        });
                    }),
                )
                .into_any_element()
        })
        .collect();

    // What the field holds, checked as it is typed.
    let text = input.read(cx).value().to_string();
    let parsed = (!text.trim().is_empty()).then(|| sandbox::sites::parse(&text));
    let verdict: Option<(IconName, Hsla, String)> = match &parsed {
        None => None,
        Some(Err(why)) => Some((IconName::CircleX, error, why.clone())),
        Some(Ok(parsed)) => {
            let allows = if parsed.site.subdomains {
                format!("Allows {} and its subdomains", parsed.site.host)
            } else {
                format!("Allows {}", parsed.site.host)
            };
            let found = check
                .as_ref()
                .filter(|check| check.host == parsed.site.host)
                .and_then(|check| check.found);
            Some(match (found, &parsed.note) {
                (Some(false), _) => (
                    IconName::TriangleAlert,
                    warning,
                    format!("No site by that name answers (typo?). {allows}."),
                ),
                (_, Some(note)) => (IconName::Info, warning, format!("{note}.")),
                (None, None) => (
                    IconName::LoaderCircle,
                    muted,
                    format!("{allows} · checking…"),
                ),
                (Some(true), None) => (IconName::CircleCheck, success, format!("{allows}.")),
            })
        }
    };
    let valid = matches!(parsed, Some(Ok(_)));
    let field = input.clone();
    let add = add_button("site-add", "Add the site")
        .disabled(!valid)
        .on_click(move |_, window, cx| add_site(&field, window, cx));
    let _ = window;
    let footer = v_flex()
        .w_full()
        .gap_1p5()
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(div().flex_1().child(Input::new(input).small()))
                .child(add),
        )
        .when_some(verdict, |this, (icon, color, text)| {
            this.child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(color)
                    .debug_selector(|| "site-verdict".into())
                    .child(Icon::new(icon).xsmall())
                    .child(text),
            )
        })
        .into_any_element();
    list_card(
        "sandbox-sites",
        "Allowed sites",
        "A site is allowed as a whole: pages can't be told apart over HTTPS. Add /* for its \
         subdomains too, like github.com/* for api.github.com.",
        rows,
        "No sites yet: commands reach nothing",
        footer,
        cx,
    )
}

/// Adds what the allowlist's field holds and empties it.
pub(super) fn add_site(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let text = input.read(cx).value().to_string();
    let added = security(cx).update(cx, |s, cx| s.add_site(&text, cx));
    if added.is_ok() {
        input.update(cx, |input, cx| input.set_value("", window, cx));
    }
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
