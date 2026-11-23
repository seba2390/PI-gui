//! The Skills page (Code mode): the skills pi loaded for the visible session, project skills it
//! skipped because the folder is not trusted, a form for new skills, and an editor for
//! `SKILL.md` files.
//!
//! The list comes from pi itself (`get_commands` reports each skill's file and scope), so it is
//! exactly what pi sees, here or on an SSH host. pi reads skills only when it starts, so every
//! change asks the window to restart pi on the same session ([`SkillsEvent::Changed`]).

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, Input, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::skills::{self, FrontMatter};
use pi_config::trust::{self, Trust};

use crate::chat::session::{Lifecycle, PiSession};
use crate::pi_place::PiPlace;
use crate::ui::{AppIcon, QuietButton as _};

/// Finds project skills (`.pi/skills` in the folder, `.agents/skills` in it and its parents up
/// to the repository root) and whether the folder has resources that need trust. Prints
/// NUL-separated records: `CWD <canonical folder>`, `TRUSTRES` (once or more), and
/// `SKILL <path> <first 4 kB>`. `$1` is the folder, `$2` the home folder.
const PROJECT_SCAN: &str = r#"cd -- "$1" 2>/dev/null || exit 0
d=$(pwd -P)
printf 'CWD\000%s\000' "$d"
h=$(cd -- "$2" 2>/dev/null && pwd -P)
for e in settings.json mcp.json extensions skills prompts themes SYSTEM.md APPEND_SYSTEM.md; do
  if [ -e "$d/.pi/$e" ]; then printf 'TRUSTRES\000'; break; fi
done
skills() {
  find "$1" -name node_modules -prune -o -name '.?*' -prune -o -type f -name SKILL.md -print 2>/dev/null |
  while IFS= read -r f; do printf 'SKILL\000%s\000' "$f"; head -c 4096 "$f"; printf '\000'; done
}
[ -d "$d/.pi/skills" ] && skills "$d/.pi/skills"
top=$(git -C "$d" rev-parse --show-toplevel 2>/dev/null)
c=$d
while :; do
  if [ -d "$c/.agents/skills" ] && [ "$c/.agents/skills" != "$h/.agents/skills" ]; then
    printf 'TRUSTRES\000'
    skills "$c/.agents/skills"
  fi
  [ "$c" = "$top" ] && break
  p=$(dirname -- "$c"); [ "$p" = "$c" ] && break; c=$p
done
exit 0"#;

/// Writes stdin to `$1/trust.json` under pi's lock (the `trust.json.lock` folder that pi's
/// proper-lockfile uses).
const TRUST_WRITE: &str = r#"f=$1/trust.json; l=$f.lock; i=0
mkdir -p -- "$1" || exit 1
while ! mkdir "$l" 2>/dev/null; do
  i=$((i + 1))
  [ "$i" -gt 50 ] && { echo "trust.json is locked by another pi process" >&2; exit 1; }
  sleep 0.1
done
trap 'rmdir "$l"' EXIT
cat > "$f.pistachio-tmp" && mv -- "$f.pistachio-tmp" "$f""#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsEvent {
    /// Skills or trust changed: restart pi so it loads them.
    Changed,
}

/// Where a skill lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Project,
    Personal,
    Other,
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    description: String,
    path: String,
    scope: Scope,
    /// pi loaded it (else: a project skill skipped because the folder is not trusted).
    loaded: bool,
    /// Part of a pi package: edit it where the package comes from.
    from_package: bool,
}

/// What the folder scan found.
#[derive(Debug, Clone, Default)]
struct ProjectScan {
    cwd: String,
    needs_trust: bool,
    trust: Option<Trust>,
    skills: Vec<(String, FrontMatter)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    None,
    New,
    Skill(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewScope {
    Personal,
    Project,
}

#[allow(clippy::struct_excessive_bools)] // independent UI flags
pub struct SkillsView {
    session: Entity<PiSession>,
    place: Option<PiPlace>,
    scan: Option<Result<ProjectScan, SharedString>>,
    selection: Selection,
    // New skill form.
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<EditorState>,
    new_scope: NewScope,
    only_on_request: bool,
    trust_too: bool,
    form_error: Option<SharedString>,
    // The open skill.
    editor: Entity<EditorState>,
    loaded_text: Option<Result<String, SharedString>>,
    detail_error: Option<SharedString>,
    confirm_trash: bool,
    busy: bool,
    notice: Option<SharedString>,
    _session_subscription: Subscription,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<SkillsEvent> for SkillsView {}

impl SkillsView {
    pub fn new(session: Entity<PiSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = session.read(cx).host().cloned();
        let subscription = cx.observe(&session, |_, _, cx| cx.notify());
        let mut this = Self {
            place: PiPlace::for_host(host.as_ref(), cx),
            session,
            scan: None,
            selection: Selection::None,
            name: cx.new(|cx| InputState::new(window, cx).placeholder("e.g. release-notes")),
            description: cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    "What it does and when to use it, e.g. \"Write release notes from git \
                     history. Use when preparing a release.\"",
                )
            }),
            instructions: cx.new(|cx| EditorState::new(window, cx).language("markdown")),
            new_scope: NewScope::Personal,
            only_on_request: false,
            trust_too: true,
            form_error: None,
            editor: cx.new(|cx| EditorState::new(window, cx).language("markdown")),
            loaded_text: None,
            detail_error: None,
            confirm_trash: false,
            busy: false,
            notice: None,
            _session_subscription: subscription,
            _tasks: Vec::new(),
        };
        this.rescan(cx);
        this
    }

    /// Follows the session after pi restarted (same folder and host).
    pub fn set_session(&mut self, session: Entity<PiSession>, cx: &mut Context<Self>) {
        self._session_subscription = cx.observe(&session, |_, _, cx| cx.notify());
        self.session = session;
        self.rescan(cx);
        cx.notify();
    }

    /// Opens the skill at `path` (its `SKILL.md`).
    pub fn open_skill(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Selection::Skill(path), window, cx);
    }

    /// Opens the new-skill form.
    pub fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Selection::New, window, cx);
    }

    /// The session the page shows skills for.
    pub fn session_entity(&self) -> &Entity<PiSession> {
        &self.session
    }

    fn cwd(&self, cx: &App) -> String {
        self.session.read(cx).cwd().to_string_lossy().into_owned()
    }

    /// Scans the folder for project skills and reads the trust store, in the background.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.session.read(cx).is_idle() {
            // No session folder yet: nothing to scan.
            self.scan = Some(Ok(ProjectScan::default()));
            return;
        }
        let Some(place) = self.place.clone() else {
            self.scan = Some(Err(
                "Connect to the host first: pi's folders there are not known yet.".into(),
            ));
            return;
        };
        let cwd = self.cwd(cx);
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { scan_project(&place, &cwd) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.scan = Some(result.map_err(SharedString::from));
                cx.notify();
            });
        });
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(task);
    }

    /// The list: skills pi loaded, then project skills it skipped.
    fn entries(&self, cx: &App) -> Vec<Entry> {
        let session = self.session.read(cx);
        let mut entries: Vec<Entry> = session
            .commands()
            .iter()
            .filter(|command| command.source == "skill")
            .map(|command| {
                let info = command.source_info.clone().unwrap_or_default();
                Entry {
                    name: command
                        .name
                        .strip_prefix("skill:")
                        .unwrap_or(&command.name)
                        .to_owned(),
                    description: command.description.clone().unwrap_or_default(),
                    scope: match info.scope.as_str() {
                        "project" => Scope::Project,
                        "user" => Scope::Personal,
                        _ => Scope::Other,
                    },
                    from_package: info.origin == "package" || info.path.contains("/node_modules/"),
                    path: info.path,
                    loaded: true,
                }
            })
            .collect();
        if let Some(Ok(scan)) = &self.scan {
            for (path, front) in &scan.skills {
                if entries.iter().any(|entry| &entry.path == path) {
                    continue;
                }
                let folder = path
                    .trim_end_matches("/SKILL.md")
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                entries.push(Entry {
                    name: front.name.clone().unwrap_or_else(|| folder.to_owned()),
                    description: front.description.clone().unwrap_or_default(),
                    path: path.clone(),
                    scope: Scope::Project,
                    loaded: false,
                    from_package: false,
                });
            }
        }
        entries.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
        entries
    }

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        self.detail_error = None;
        self.confirm_trash = false;
        self.notice = None;
        if let Selection::Skill(path) = &selection {
            self.load_skill(path.clone(), cx);
        }
        if selection == Selection::New {
            self.form_error = None;
            self.name.update(cx, |input, cx| input.focus(window, cx));
        }
        self.selection = selection;
        cx.notify();
    }

    fn load_skill(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        self.loaded_text = None;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    place
                        .read(&path)
                        .and_then(|text| text.ok_or_else(|| format!("{path} no longer exists.")))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(text) = &result {
                    let (editor, text) = (this.editor.clone(), text.clone());
                    cx.defer(move |cx| {
                        if let Some(window) = cx.active_window() {
                            let _ = window.update(cx, |_, window, cx| {
                                editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
                            });
                        }
                    });
                }
                this.loaded_text = Some(result.map_err(SharedString::from));
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Runs `work` in the background, then reports success (rescanning and asking for a pi
    /// restart) or shows the error with `on_error`.
    fn run(
        &mut self,
        work: impl FnOnce(&PiPlace) -> Result<Option<Selection>, String> + Send + 'static,
        on_error: fn(&mut Self, SharedString),
        cx: &mut Context<Self>,
    ) {
        let Some(place) = self.place.clone() else {
            return;
        };
        self.busy = true;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&place) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(selection) => {
                        if let Some(selection) = selection {
                            if let Selection::Skill(path) = &selection {
                                this.load_skill(path.clone(), cx);
                            }
                            this.selection = selection;
                        }
                        this.confirm_trash = false;
                        this.rescan(cx);
                        cx.emit(SkillsEvent::Changed);
                    }
                    Err(error) => on_error(this, error.into()),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        let description = self.description.read(cx).value().trim().to_owned();
        let body = self.instructions.read(cx).value().to_string();
        if let Err(error) =
            skills::validate_name(&name).and_then(|()| skills::validate_description(&description))
        {
            self.form_error = Some(error.to_string().into());
            cx.notify();
            return;
        }
        let Some(place) = &self.place else { return };
        let (path, trust_folder) = match self.new_scope {
            NewScope::Personal => (place.agent_path(&format!("skills/{name}/SKILL.md")), None),
            NewScope::Project => {
                let scan = match &self.scan {
                    Some(Ok(scan)) => Some(scan.clone()),
                    _ => None,
                };
                let cwd = scan
                    .as_ref()
                    .map_or_else(|| self.cwd(cx), |scan| scan.cwd.clone());
                let untrusted = !scan
                    .as_ref()
                    .and_then(|scan| scan.trust.as_ref())
                    .is_some_and(Trust::is_trusted);
                (
                    format!("{cwd}/.pi/skills/{name}/SKILL.md"),
                    (untrusted && self.trust_too).then_some(cwd),
                )
            }
        };
        let text = skills::render_skill(&name, &description, self.only_on_request, &body);
        self.form_error = None;
        self.run(
            move |place| {
                if place.read(&path)?.is_some() {
                    return Err(format!("A skill named {name} already exists there."));
                }
                place.write(&path, &text, false)?;
                if let Some(folder) = trust_folder {
                    write_trust(place, &folder, Some(true))?;
                }
                Ok(Some(Selection::Skill(path)))
            },
            |this, error| this.form_error = Some(error),
            cx,
        );
    }

    fn save_skill(&mut self, path: String, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        if let Err(error) = skills::validate_skill_file(&text) {
            self.detail_error = Some(error.to_string().into());
            cx.notify();
            return;
        }
        self.detail_error = None;
        self.notice = Some("Saved. pi restarts to load the change.".into());
        self.run(
            move |place| place.write(&path, &text, false).map(|()| None),
            |this, error| {
                this.notice = None;
                this.detail_error = Some(error);
            },
            cx,
        );
    }

    fn set_only_on_request(
        &mut self,
        path: String,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = skills::set_only_on_request(&self.editor.read(cx).value(), on);
        let shown = text.clone();
        self.editor
            .update(cx, |editor, cx| editor.set_value(shown, window, cx));
        self.run(
            move |place| place.write(&path, &text, false).map(|()| None),
            |this, error| this.detail_error = Some(error),
            cx,
        );
    }

    fn trash_skill(&mut self, path: String, cx: &mut Context<Self>) {
        // A skill is its folder; a loose markdown skill is just the file.
        let target = path
            .strip_suffix("/SKILL.md")
            .map_or_else(|| path.clone(), str::to_owned);
        self.run(
            move |place| place.trash(&target).map(|()| Some(Selection::None)),
            |this, error| this.detail_error = Some(error),
            cx,
        );
    }

    fn set_trust(&mut self, trusted: bool, cx: &mut Context<Self>) {
        let Some(Ok(scan)) = &self.scan else { return };
        let folder = scan.cwd.clone();
        self.run(
            move |place| write_trust(place, &folder, Some(trusted)).map(|()| None),
            |this, error| this.notice = Some(error),
            cx,
        );
    }

    // ---- rendering -------------------------------------------------------------

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, foreground, hover, active) = (
            theme.muted_foreground,
            theme.foreground,
            theme.secondary_hover,
            theme.accent,
        );
        let starting = self.session.read(cx).lifecycle() == &Lifecycle::Starting;
        let entries = self.entries(cx);
        let mut list = v_flex().gap_0p5();
        let mut current: Option<Scope> = None;
        for entry in &entries {
            if current != Some(entry.scope) {
                current = Some(entry.scope);
                list = list.child(
                    div()
                        .px_2()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .text_color(muted)
                        .child(match entry.scope {
                            Scope::Project => "This project",
                            Scope::Personal => "Personal (every project)",
                            Scope::Other => "From settings and packages",
                        }),
                );
            }
            let selected = self.selection == Selection::Skill(entry.path.clone());
            let path = entry.path.clone();
            list = list.child(
                v_flex()
                    .id(ElementId::Name(format!("skill:{}", entry.path).into()))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(active))
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(Selection::Skill(path.clone()), window, cx);
                    }))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(foreground)
                                    .truncate()
                                    .child(entry.name.clone()),
                            )
                            .when(!entry.loaded, |this| {
                                this.child(div().text_xs().text_color(muted).child("· not loaded"))
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(entry.description.clone()),
                    ),
            );
        }
        if entries.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .pt_3()
                    .text_xs()
                    .text_color(muted)
                    .child(if starting {
                        "pi is starting…"
                    } else {
                        "No skills yet."
                    }),
            );
        }
        v_flex()
            .w(px(280.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div().p_2().child(
                    Button::new("new-skill")
                        .quiet(cx)
                        .w_full()
                        .justify_start()
                        .icon(IconName::Plus)
                        .label("New skill")
                        .selected(self.selection == Selection::New)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.select(Selection::New, window, cx);
                        })),
                ),
            )
            .children(self.render_trust(cx))
            .child(
                div()
                    .id("skills-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_3()
                    .child(list),
            )
    }

    /// The project's trust state when it matters (it has project resources).
    fn render_trust(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.session.read(cx).is_idle() {
            return None;
        }
        let Some(Ok(scan)) = &self.scan else {
            return None;
        };
        let trust = scan.trust.clone()?;
        if !scan.needs_trust {
            return None;
        }
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let colors = crate::theme::palette(cx);
        let body = if trust.is_trusted() {
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_xs()
                        .child(div().size(px(7.)).rounded_full().bg(colors.success))
                        .child(match &trust {
                            Trust::Saved { path, .. } if *path != scan.cwd => {
                                format!("Trusted (through {path})")
                            }
                            Trust::DefaultAlways => "Trusted (defaultProjectTrust: always)".into(),
                            _ => "This project is trusted".into(),
                        }),
                )
                .when(
                    matches!(trust, Trust::Saved { ref path, .. } if *path == scan.cwd),
                    |this| {
                        this.child(
                            Button::new("untrust")
                                .quiet(cx)
                                .xsmall()
                                .label("Stop trusting")
                                .on_click(cx.listener(|this, _, _, cx| this.set_trust(false, cx))),
                        )
                    },
                )
        } else {
            v_flex()
                .gap_1p5()
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_xs()
                        .child(div().size(px(7.)).rounded_full().bg(colors.warning))
                        .child(if matches!(trust, Trust::Saved { trusted: false, .. }) {
                            "You chose not to trust this project"
                        } else {
                            "This project is not trusted"
                        }),
                )
                .child(div().text_xs().text_color(muted).child(
                    "pi skips its project skills, prompts, extensions and .pi settings until \
                     you trust it. Trust only folders whose contents you know: extensions run \
                     code.",
                ))
                .child(
                    Button::new("trust")
                        .small()
                        .label("Trust this project")
                        .on_click(cx.listener(|this, _, _, cx| this.set_trust(true, cx))),
                )
        };
        Some(
            div()
                .mx_2()
                .mb_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let error_color = crate::theme::palette(cx).error;
        let untrusted = match &self.scan {
            Some(Ok(scan)) => !scan.trust.as_ref().is_some_and(Trust::is_trusted),
            _ => true,
        };
        let idle = self.session.read(cx).is_idle();
        let label = |text: &'static str| div().text_xs().text_color(muted).child(text);
        let scope_button =
            |id: &'static str, scope: NewScope, text: &'static str, cx: &mut Context<Self>| {
                Button::new(id)
                    .small()
                    .label(text)
                    .selected(self.new_scope == scope)
                    .when(self.new_scope == scope, |this| this.primary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_scope = scope;
                        cx.notify();
                    }))
            };
        let (only, trust_too) = (self.only_on_request, self.trust_too);
        v_flex()
            .gap_3()
            .child(div().text_lg().font_weight(FontWeight::MEDIUM).child("New skill"))
            .child(div().text_sm().text_color(muted).child(
                "A skill gives pi instructions for one kind of task. pi sees only its name and \
                 description, and reads the instructions when a task matches (or when you type \
                 /skill:name).",
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Name"))
                    .child(Input::new(&self.name).small())
                    .child(div().text_xs().text_color(muted).child(
                        "Lowercase letters, digits and hyphens, at most 64 characters.",
                    )),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Description: what it does and when to use it"))
                    .child(Input::new(&self.description).small()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Save in"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(scope_button("scope-personal", NewScope::Personal, "Personal: every project", cx))
                            .when(!idle, |this| {
                                this.child(scope_button(
                                    "scope-project",
                                    NewScope::Project,
                                    "This project (.pi/skills)",
                                    cx,
                                ))
                            }),
                    )
                    .when(self.new_scope == NewScope::Project && untrusted, |this| {
                        this.child(
                            Checkbox::new("trust-too")
                                .small()
                                .checked(trust_too)
                                .label("Also trust this project (pi loads project skills only in trusted folders)")
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.trust_too = *checked;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Instructions (Markdown)"))
                    .child(
                        div()
                            .h(px(220.))
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .overflow_hidden()
                            .child(Editor::new(&self.instructions).size_full()),
                    ),
            )
            .child(
                Checkbox::new("only-on-request")
                    .small()
                    .checked(only)
                    .label("Only when I call it with /skill:name (pi will not load it by itself)")
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.only_on_request = *checked;
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.form_error.clone(), |this, error| {
                        this.child(div().flex_1().text_xs().text_color(error_color).child(error))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("create-skill")
                            .primary()
                            .label(if self.busy { "Creating…" } else { "Create skill" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_skill(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let colors = crate::theme::palette(cx);
        let entry = self
            .entries(cx)
            .into_iter()
            .find(|entry| entry.path == path);
        let home = cx.global::<crate::config::AppConfig>().home.clone();
        let shown_path = self.place.as_ref().map_or_else(
            || path.to_owned(),
            |place| place.describe(path, home.as_deref()),
        );
        let text = match &self.loaded_text {
            None => {
                return div()
                    .text_sm()
                    .text_color(muted)
                    .child("Reading SKILL.md…")
                    .into_any_element();
            }
            Some(Err(error)) => {
                return div()
                    .text_sm()
                    .text_color(colors.error)
                    .child(error.clone())
                    .into_any_element();
            }
            Some(Ok(text)) => text,
        };
        let front = skills::parse_front_matter(text);
        let name = entry
            .as_ref()
            .map(|entry| entry.name.clone())
            .or(front.name.clone())
            .unwrap_or_default();
        let read_only = entry.as_ref().is_some_and(|entry| entry.from_package);
        let loaded = entry.as_ref().is_none_or(|entry| entry.loaded);
        let only = skills::parse_front_matter(&self.editor.read(cx).value()).only_on_request;
        let (toggle_path, save_path, trash_path) =
            (path.to_owned(), path.to_owned(), path.to_owned());
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::from(AppIcon::Sparkles).small().text_color(muted))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::MEDIUM)
                            .child(name.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("/skill:{name}")),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(shown_path),
            )
            .when(!loaded, |this| {
                this.child(div().text_xs().text_color(colors.warning).child(
                    "pi does not load this skill: the project is not trusted (see the left).",
                ))
            })
            .when(read_only, |this| {
                this.child(div().text_xs().text_color(muted).child(
                    "This skill comes from a pi package; change it where the package comes from.",
                ))
            })
            .when(!read_only, |this| {
                this.child(
                    Checkbox::new("skill-only-on-request")
                        .small()
                        .checked(only)
                        .label("Only when I call it with /skill:name")
                        .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                            this.set_only_on_request(toggle_path.clone(), *checked, window, cx);
                        })),
                )
            })
            .child(
                div()
                    .h(px(420.))
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(Editor::new(&self.editor).size_full()),
            )
            .when(!read_only, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .when_some(self.detail_error.clone(), |this, error| {
                            this.child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(colors.error)
                                    .child(error),
                            )
                        })
                        .when_some(self.notice.clone(), |this, notice| {
                            this.child(div().flex_1().text_xs().text_color(muted).child(notice))
                        })
                        .child(div().flex_1())
                        .map(|this| {
                            if self.confirm_trash {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("Move this skill to the trash?"),
                                )
                                .child(
                                    Button::new("trash-yes")
                                        .small()
                                        .with_variant(ButtonVariant::Danger)
                                        .label("Move to trash")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.trash_skill(trash_path.clone(), cx);
                                        })),
                                )
                            } else {
                                this.child(
                                    Button::new("trash-skill")
                                        .quiet(cx)
                                        .small()
                                        .icon(IconName::Delete)
                                        .label("Move to trash")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_trash = true;
                                            cx.notify();
                                        })),
                                )
                            }
                        })
                        .child(
                            Button::new("save-skill")
                                .primary()
                                .small()
                                .label(if self.busy { "Saving…" } else { "Save" })
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_skill(save_path.clone(), cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_intro(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let error = match &self.scan {
            Some(Err(error)) => Some(error.clone()),
            _ => None,
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(
                Icon::from(AppIcon::Sparkles)
                    .size_8()
                    .text_color(crate::theme::palette(cx).accent),
            )
            .child(div().text_xl().child("Skills"))
            .child(
                div()
                    .max_w(px(480.))
                    .text_sm()
                    .text_color(muted)
                    .text_center()
                    .child(
                        "Skills teach pi how to do a kind of task: instructions, plus scripts or \
                         references next to them. pi loads a skill when a task matches its \
                         description, or when you type /skill:name.",
                    ),
            )
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(crate::theme::palette(cx).error)
                        .child(error),
                )
            })
            .child(
                Button::new("intro-new-skill")
                    .primary()
                    .icon(IconName::Plus)
                    .label("New skill")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.select(Selection::New, window, cx)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SkillsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("skills");
        let detail = match self.selection.clone() {
            Selection::None => self.render_intro(cx),
            Selection::New => self.render_new(cx),
            Selection::Skill(path) => self.render_skill(&path, cx),
        };
        h_flex()
            .size_full()
            .items_start()
            .bg(cx.theme().background)
            .child(self.render_list(cx))
            .child(
                div()
                    .id("skill-detail")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(div().w_full().max_w(px(760.)).mx_auto().p_5().child(detail)),
            )
    }
}

/// Scans `cwd` (see [`PROJECT_SCAN`]) and reads pi's trust store and default.
fn scan_project(place: &PiPlace, cwd: &str) -> Result<ProjectScan, String> {
    let home = place.home();
    let stdout = place.run_script(PROJECT_SCAN, &[cwd, &home], None)?;
    let mut scan = parse_scan(&stdout);
    if scan.cwd.is_empty() {
        cwd.clone_into(&mut scan.cwd);
    }
    let store = place
        .read(&place.agent_path("trust.json"))?
        .map(|text| trust::parse_store(&text).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let default = place
        .read(&place.agent_path("settings.json"))?
        .and_then(|text| trust::default_project_trust(&text));
    scan.trust = Some(trust::trust_of(&store, &scan.cwd, default.as_deref()));
    Ok(scan)
}

fn parse_scan(stdout: &[u8]) -> ProjectScan {
    let fields: Vec<String> = stdout
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    let mut scan = ProjectScan::default();
    let mut index = 0;
    while index < fields.len() {
        match fields[index].as_str() {
            "CWD" if index + 1 < fields.len() => {
                scan.cwd.clone_from(&fields[index + 1]);
                index += 2;
            }
            "TRUSTRES" => {
                scan.needs_trust = true;
                index += 1;
            }
            "SKILL" if index + 2 < fields.len() => {
                let path = fields[index + 1].clone();
                if !scan.skills.iter().any(|(known, _)| *known == path) {
                    scan.skills
                        .push((path, skills::parse_front_matter(&fields[index + 2])));
                }
                index += 3;
            }
            _ => index += 1,
        }
    }
    scan
}

/// Saves `decision` for `folder` in pi's `trust.json` (read, changed, written under pi's lock).
fn write_trust(place: &PiPlace, folder: &str, decision: Option<bool>) -> Result<(), String> {
    let store = place
        .read(&place.agent_path("trust.json"))?
        .map(|text| trust::parse_store(&text).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let text = trust::set_trust(&store, folder, decision);
    let agent = place.agent_path("");
    place
        .run_script(
            TRUST_WRITE,
            &[agent.trim_end_matches('/')],
            Some(text.into_bytes()),
        )
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::{PROJECT_SCAN, PiPlace, parse_scan, scan_project, write_trust};
    use std::fs;

    #[test]
    fn scan_finds_project_skills_and_trust_needs() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let repo = root.join("repo");
        let cwd = repo.join("sub");
        for dir in [
            cwd.join(".pi/skills/one"),
            repo.join(".agents/skills/two"),
            repo.join(".agents/skills/node_modules/x"),
            home.join(".agents/skills/mine"),
            root.join(".agents/skills/outside"),
        ] {
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: n\ndescription: In {}\n---\n", dir.display()),
            )
            .unwrap();
        }
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        let place = PiPlace::Local {
            agent_dir: root.join("agent"),
        };
        let stdout = place
            .run_script(
                PROJECT_SCAN,
                &[cwd.to_str().unwrap(), home.to_str().unwrap()],
                None,
            )
            .unwrap();
        let scan = parse_scan(&stdout);
        assert_eq!(scan.cwd, cwd.to_str().unwrap());
        assert!(scan.needs_trust);
        let paths: Vec<&str> = scan.skills.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                cwd.join(".pi/skills/one/SKILL.md").to_str().unwrap(),
                repo.join(".agents/skills/two/SKILL.md").to_str().unwrap(),
            ],
            "stops at the repository root and skips node_modules"
        );
        assert!(
            scan.skills[0]
                .1
                .description
                .as_deref()
                .unwrap()
                .starts_with("In ")
        );

        // Outside a repository pi looks in every parent, like this scan.
        let plain = root.join("plain");
        fs::create_dir_all(&plain).unwrap();
        let stdout = place
            .run_script(
                PROJECT_SCAN,
                &[plain.to_str().unwrap(), home.to_str().unwrap()],
                None,
            )
            .unwrap();
        let scan = parse_scan(&stdout);
        let paths: Vec<&str> = scan.skills.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [root
                .join(".agents/skills/outside/SKILL.md")
                .to_str()
                .unwrap()]
        );

        // A folder with nothing of the kind anywhere above it needs no trust.
        let lone = tempfile::tempdir().unwrap();
        let stdout = place
            .run_script(
                PROJECT_SCAN,
                &[lone.path().to_str().unwrap(), home.to_str().unwrap()],
                None,
            )
            .unwrap();
        let scan = parse_scan(&stdout);
        assert!(!scan.needs_trust || !scan.skills.is_empty());
    }

    /// With real pi (`pi` on PATH): a personal and a project skill written the way the page
    /// writes them are not loaded by a running pi, but after trusting the project (as the page
    /// does) and restarting pi, both are, with their files and scopes, and the page lists them.
    /// `cargo test -p pistachio -- --ignored skills_load_after_restart`
    #[cfg(unix)]
    #[gpui_kit::test]
    #[ignore = "needs pi on PATH"]
    async fn skills_load_after_restart(cx: &mut gpui_kit::TestAppContext) {
        use crate::chat::session::{Lifecycle, PiSession};
        use crate::config::AppConfig;
        use gpui_kit::AppContext as _;
        use std::time::{Duration, Instant};

        cx.executor().allow_parking();
        cx.update(gpui_kit::component::init);
        let agent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let project = project.path().canonicalize().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&project)
                .status()
                .unwrap()
                .success()
        );
        let mut config = AppConfig::for_tests("pi".into());
        config.child_env = vec![("PI_CODING_AGENT_DIR".into(), agent.path().into())];
        cx.update(|cx| cx.set_global(config));
        let place = PiPlace::Local {
            agent_dir: agent.path().into(),
        };

        async fn ready(session: &gpui_kit::Entity<PiSession>, cx: &mut gpui_kit::TestAppContext) {
            let deadline = Instant::now() + Duration::from_secs(60);
            while cx.read(|cx| session.read(cx).lifecycle() == &Lifecycle::Starting)
                || cx.read(|cx| session.read(cx).commands().is_empty())
            {
                assert!(Instant::now() < deadline, "pi did not start");
                cx.background_executor
                    .timer(Duration::from_millis(50))
                    .await;
            }
        }
        let skill_names = |session: &gpui_kit::Entity<PiSession>,
                           cx: &mut gpui_kit::TestAppContext| {
            cx.read(|cx| {
                let mut names: Vec<(String, String)> = session
                    .read(cx)
                    .commands()
                    .iter()
                    .filter(|command| command.source == "skill")
                    .map(|command| {
                        (
                            command.name.clone(),
                            command.source_info.clone().unwrap_or_default().scope,
                        )
                    })
                    .collect();
                names.sort();
                names
            })
        };

        let personal = place.agent_path("skills/release-notes/SKILL.md");
        let project_skill = format!("{}/.pi/skills/repo-guide/SKILL.md", project.display());
        let first = cx.new(|cx| PiSession::new(project.clone(), None, None, cx));
        ready(&first, cx).await;
        assert_eq!(skill_names(&first, cx), Vec::<(String, String)>::new());

        place
            .write(
                &personal,
                &pi_config::skills::render_skill(
                    "release-notes",
                    "Write release notes. Use when preparing a release.",
                    false,
                    "Read git log.",
                ),
                false,
            )
            .unwrap();
        place
            .write(
                &project_skill,
                &pi_config::skills::render_skill(
                    "repo-guide",
                    "How this repository works. Use for any change here.",
                    true,
                    "",
                ),
                false,
            )
            .unwrap();
        // Before trusting: a restarted pi loads the personal skill only.
        drop(first);
        let second = cx.new(|cx| PiSession::new(project.clone(), None, None, cx));
        ready(&second, cx).await;
        assert_eq!(
            skill_names(&second, cx),
            [("skill:release-notes".to_owned(), "user".to_owned())]
        );
        let scan = scan_project(&place, project.to_str().unwrap()).unwrap();
        assert!(scan.needs_trust);
        assert!(!scan.trust.as_ref().unwrap().is_trusted());
        assert_eq!(scan.skills.len(), 1);

        write_trust(&place, &scan.cwd, Some(true)).unwrap();
        drop(second);
        let third = cx.new(|cx| PiSession::new(project.clone(), None, None, cx));
        ready(&third, cx).await;
        assert_eq!(
            skill_names(&third, cx),
            [
                ("skill:release-notes".to_owned(), "user".to_owned()),
                ("skill:repo-guide".to_owned(), "project".to_owned()),
            ]
        );
        let paths: Vec<String> = cx.read(|cx| {
            third
                .read(cx)
                .commands()
                .iter()
                .filter_map(|command| Some(command.source_info.clone()?.path))
                .collect()
        });
        assert!(
            paths.contains(&personal) && paths.contains(&project_skill),
            "{paths:?}"
        );

        // The page lists both as loaded.
        let (view, cx) =
            cx.add_window_view(|window, cx| super::SkillsView::new(third.clone(), window, cx));
        let entries = cx.read(|cx| view.read(cx).entries(cx));
        let shown: Vec<(&str, bool)> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.loaded))
            .collect();
        assert_eq!(shown, [("repo-guide", true), ("release-notes", true)]);
    }

    /// The scan, skill files and trust writes over SSH, in a scratch folder on the host (not
    /// its real pi settings). `PISTACHIO_SSH_TEST_HOST=<alias> cargo test -p pistachio -- --ignored skills_over_ssh`
    #[test]
    #[ignore = "needs PISTACHIO_SSH_TEST_HOST"]
    fn skills_over_ssh() {
        let Ok(alias) = std::env::var("PISTACHIO_SSH_TEST_HOST") else {
            return;
        };
        let ssh = std::sync::Arc::new(remote::Ssh::new(remote::Target::Alias {
            alias: alias.clone(),
        }));
        ssh.connect(false).unwrap();
        let root = ssh
            .exec("d=$(mktemp -d /tmp/pistachio-skills.XXXXXX) && cd \"$d\" && mkdir proj && (cd proj && git init -q) && pwd -P")
            .unwrap()
            .trim()
            .to_owned();
        let place = PiPlace::Remote {
            label: alias,
            ssh: ssh.clone(),
            agent_dir: format!("{root}/agent"),
            home: format!("{root}/home"),
        };
        let result = std::panic::catch_unwind(|| {
            let cwd = format!("{root}/proj");
            let skill = format!("{cwd}/.pi/skills/repo-guide/SKILL.md");
            place
                .write(
                    &skill,
                    &pi_config::skills::render_skill("repo-guide", "Guide. Use here.", false, ""),
                    false,
                )
                .unwrap();
            let scan = scan_project(&place, &cwd).unwrap();
            assert_eq!(scan.cwd, cwd);
            assert!(scan.needs_trust);
            assert!(!scan.trust.as_ref().unwrap().is_trusted());
            assert_eq!(scan.skills[0].0, skill);
            assert_eq!(scan.skills[0].1.name.as_deref(), Some("repo-guide"));
            write_trust(&place, &cwd, Some(true)).unwrap();
            assert!(
                scan_project(&place, &cwd)
                    .unwrap()
                    .trust
                    .unwrap()
                    .is_trusted()
            );
            place
                .trash(&format!("{cwd}/.pi/skills/repo-guide"))
                .unwrap();
            assert_eq!(scan_project(&place, &cwd).unwrap().skills.len(), 0);
        });
        let _ = ssh.exec(&format!("rm -rf {}", remote::sh_quote(&root)));
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn trust_round_trip_through_the_store() {
        let root = tempfile::tempdir().unwrap();
        let place = PiPlace::Local {
            agent_dir: root.path().join("agent"),
        };
        write_trust(&place, "/srv/app", Some(true)).unwrap();
        write_trust(&place, "/other", Some(false)).unwrap();
        let text = fs::read_to_string(root.path().join("agent/trust.json")).unwrap();
        assert_eq!(text, "{\n  \"/other\": false,\n  \"/srv/app\": true\n}\n");
        assert!(!root.path().join("agent/trust.json.lock").exists());
        let scan = scan_project(&place, "/srv/app/sub").unwrap();
        assert!(scan.trust.unwrap().is_trusted());
    }
}
