//! The Skills page (Code mode): the skills pi loaded for the visible session, project skills it
//! skipped because the folder is not trusted, a form for new skills, and an editor for
//! `SKILL.md` files.
//!
//! The list comes from pi itself (`get_commands` reports each skill's file and scope), so it is
//! exactly what pi sees, here or on an SSH host. Until pi runs, the skills that come with the app
//! are listed (it will load them). pi reads skills only when it starts, so every
//! change asks the window to restart pi on the same session ([`SkillsEvent::Changed`]).

mod look;

use std::collections::HashSet;

use gpui_kit::base::spring;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::skills::{self, FrontMatter};
use pi_config::trust::{self, Trust};

use crate::builtin_skills;
use crate::chat::session::{Lifecycle, PiSession};
use crate::config::{AppConfig, RecentFolders};
use crate::pi_place::PiPlace;
use crate::ui::{AppIcon, PiField, QuietButton as _, buttons};

/// How wide the list is beside an open skill.
const LIST_WIDTH: Pixels = px(288.);

/// What the instructions of a new skill start with.
const TEMPLATE: &str = "\
# What this skill does

One or two sentences on the goal, and what a good result looks like.

## When to use it

- Requests that should use this skill.
- Requests that look similar but should not.

## Steps

1. What to do first, with the exact command or file.
2. What to do next.
3. How to check that it worked.

## Good to know

- Paths, commands and conventions pi would not know by itself.
- Mistakes to avoid, and why.

## Example

Show a short example of the output, if its format matters.
";

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

/// Finds the skills in the libraries: `$1` and `$2` are the folders of the user's own, the rest
/// are projects (`.pi/skills` and `.agents/skills` in each). Prints NUL-separated records:
/// `SKILL <project, or nothing> <path> <first 4 kB>`.
const LIBRARY_SCAN: &str = r#"skills() {
  [ -d "$2" ] || return
  find "$2" -name node_modules -prune -o -name '.?*' -prune -o -type f -name SKILL.md -print 2>/dev/null |
  while IFS= read -r f; do printf 'SKILL\000%s\000%s\000' "$1" "$f"; head -c 4096 "$f"; printf '\000'; done
}
own=$2
skills '' "$1"; skills '' "$2"; shift 2
for p in "$@"; do
  d=$(cd -- "$p" 2>/dev/null && pwd -P) || continue
  skills "$d" "$d/.pi/skills"
  [ "$d/.agents/skills" = "$own" ] || skills "$d" "$d/.agents/skills"
done
exit 0"#;

/// Prints the first 4 kB of each file in `$@`, and a NUL after each (also a missing one).
const READ_HEADS: &str = r#"for f in "$@"; do head -c 4096 "$f" 2>/dev/null; printf '\000'; done"#;

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
    BuiltIn,
}

/// Where the page shows a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Library {
    BuiltIn,
    Own,
    Project,
}

impl Library {
    const ALL: [Self; 3] = [Self::BuiltIn, Self::Own, Self::Project];

    fn of(scope: Scope) -> Self {
        match scope {
            Scope::BuiltIn => Self::BuiltIn,
            Scope::Personal | Scope::Other => Self::Own,
            Scope::Project => Self::Project,
        }
    }
}

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    description: String,
    path: String,
    scope: Scope,
    /// The project it is only for.
    project: Option<String>,
    /// pi loaded it or will (else: it skipped the skill, such as a project's that is not
    /// trusted).
    loaded: bool,
    /// Part of a pi package: edit it where the package comes from.
    from_package: bool,
    /// pi does not load it by itself, only when the user calls it.
    on_request: bool,
}

impl Entry {
    /// Comes with Pistachio, which rewrites it whenever a session starts.
    fn builtin(&self) -> bool {
        self.scope == Scope::BuiltIn
    }

    fn is_read_only(&self) -> bool {
        self.from_package || self.builtin()
    }
}

/// A skill found in a library.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    project: Option<String>,
    path: String,
    front: FrontMatter,
}

impl Found {
    fn name(&self) -> String {
        name_of(&self.path, &self.front)
    }
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
    /// The skills in the user's own library and the projects', whether or not pi runs.
    found: Vec<Found>,
    /// Counts the scans, so that an overtaken one is dropped.
    scans: usize,
    /// The skills pi reported that are only loaded on request (pi does not say), and the files
    /// looked at for that.
    on_request: HashSet<String>,
    looked_at: Vec<String>,
    selection: Selection,
    // New skill form.
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<EditorState>,
    new_scope: NewScope,
    /// The project chosen for a new skill (else the session's).
    project: Option<String>,
    only_on_request: bool,
    trust_too: bool,
    form_error: Option<SharedString>,
    // The open skill.
    editor: Entity<EditorState>,
    loaded_text: Option<Result<String, SharedString>>,
    detail_error: Option<SharedString>,
    /// What is being written to the open skill.
    writing: Option<String>,
    confirm_trash: bool,
    busy: bool,
    notice: Option<SharedString>,
    /// A skill was created: the form starts empty again.
    reset_form: bool,
    window: AnyWindowHandle,
    /// Behind the libraries.
    pi_field: Entity<PiField>,
    _session_subscription: Subscription,
    _editor_subscription: Subscription,
    _recent_subscription: Subscription,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<SkillsEvent> for SkillsView {}

impl SkillsView {
    pub fn new(session: Entity<PiSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = session.read(cx).host().cloned();
        let subscription = cx.observe(&session, Self::session_changed);
        let editor = cx.new(|cx| EditorState::new(window, cx).language("markdown"));
        // Save depends on whether something was changed.
        let editor_subscription = cx.subscribe(&editor, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut this = Self {
            place: PiPlace::for_host(host.as_ref(), cx),
            session,
            scan: None,
            found: Vec::new(),
            scans: 0,
            on_request: HashSet::new(),
            looked_at: Vec::new(),
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
            project: None,
            only_on_request: false,
            trust_too: true,
            form_error: None,
            editor,
            loaded_text: None,
            detail_error: None,
            writing: None,
            confirm_trash: false,
            busy: false,
            notice: None,
            reset_form: false,
            window: window.window_handle(),
            pi_field: cx.new(|_| PiField::plain()),
            _session_subscription: subscription,
            _editor_subscription: editor_subscription,
            // The projects are known a moment after the app opened.
            _recent_subscription: cx.observe_global::<RecentFolders>(Self::scan_libraries),
            _tasks: Vec::new(),
        };
        this.rescan(cx);
        this
    }

    /// Follows the session after pi restarted (same folder and host).
    pub fn set_session(&mut self, session: Entity<PiSession>, cx: &mut Context<Self>) {
        self._session_subscription = cx.observe(&session, Self::session_changed);
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

    fn session_changed(&mut self, _: Entity<PiSession>, cx: &mut Context<Self>) {
        self.look_at_skills(cx);
        cx.notify();
    }

    /// Reads, in the background, whether the skills pi reported are only loaded on request,
    /// once for each set of skills.
    fn look_at_skills(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = (self.session.read(cx).commands().iter())
            .filter(|command| command.source == "skill")
            .filter_map(|command| Some(command.source_info.clone()?.path))
            .collect();
        if paths == self.looked_at {
            return;
        }
        let Some(place) = self.place.clone() else {
            return;
        };
        self.looked_at.clone_from(&paths);
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { only_on_request(&place, paths) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match found {
                    Ok(found) => this.on_request = found,
                    Err(error) => log::info!("could not read the skills: {error}"),
                }
                cx.notify();
            });
        });
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(task);
    }

    /// The session's own folder.
    fn session_project(&self, cx: &App) -> Option<String> {
        if self.session.read(cx).is_idle() {
            return None;
        }
        match &self.scan {
            Some(Ok(scan)) if !scan.cwd.is_empty() => Some(scan.cwd.clone()),
            _ => Some(self.cwd(cx)),
        }
    }

    /// The project a new skill is for, when it is for one.
    fn chosen_project(&self, cx: &App) -> Option<String> {
        self.project.clone().or_else(|| self.session_project(cx))
    }

    /// Whether pi is known to trust the project (only the session's was looked up).
    fn is_trusted(&self, project: &str) -> bool {
        matches!(&self.scan, Some(Ok(scan))
            if scan.cwd == project && scan.trust.as_ref().is_some_and(Trust::is_trusted))
    }

    /// A folder for display: `~/…`, also on a host.
    fn describe(&self, folder: &str, cx: &App) -> String {
        let home = match &self.place {
            Some(place) => Some(std::path::PathBuf::from(place.home())),
            None => cx.global::<AppConfig>().home.clone(),
        };
        crate::ui::short_path(std::path::Path::new(folder), home.as_deref())
    }

    /// The projects to choose from: the session's, then the ones used lately, there.
    fn projects(&self, cx: &App) -> Vec<String> {
        let mut projects: Vec<String> = (self.chosen_project(cx).into_iter())
            .chain(self.session_project(cx))
            .collect();
        match self.session.read(cx).host() {
            Some(host) => {
                let hosts = crate::hosts::hosts(cx);
                let hosts = hosts.read(cx);
                projects.extend(
                    hosts
                        .get(host)
                        .map(|host| host.recent_folders.clone())
                        .unwrap_or_default(),
                );
                // The folders sessions ran in there.
                projects.extend(
                    (hosts.sessions(host).iter())
                        .map(|session| session.cwd.to_string_lossy().into_owned()),
                );
            }
            None => projects.extend(
                (cx.try_global::<RecentFolders>()
                    .into_iter()
                    .flat_map(|recent| &recent.0))
                .map(|folder| folder.to_string_lossy().into_owned()),
            ),
        }
        let mut seen = HashSet::new();
        projects.retain(|project| seen.insert(project.clone()));
        projects
    }

    fn choose_project(&mut self, project: String, cx: &mut Context<Self>) {
        self.project = Some(project);
        self.new_scope = NewScope::Project;
        self.form_error = None;
        cx.notify();
    }

    /// Asks for a project's folder on this computer.
    fn pick_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project".into()),
        });
        let task = cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(folder) = paths.pop()
            {
                let _ = this.update(cx, |this, cx| {
                    this.choose_project(folder.to_string_lossy().into_owned(), cx);
                    this.scan_libraries(cx);
                });
            }
        });
        self._tasks.push(task);
    }

    /// Scans the folder for project skills and reads the trust store, in the background.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        self.looked_at.clear();
        self.look_at_skills(cx);
        self.scan_libraries(cx);
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

    /// Finds the skills in the user's own library and in the projects, in the background.
    fn scan_libraries(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        let home = match &place {
            PiPlace::Local { .. } => (cx.global::<AppConfig>().home.as_ref())
                .map(|home| home.to_string_lossy().into_owned()),
            PiPlace::Remote { home, .. } => Some(home.clone()),
        };
        let projects = self.projects(cx);
        self.scans += 1;
        let scan = self.scans;
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { find_skills(&place, home.as_deref(), &projects) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.scans != scan {
                    return;
                }
                match found {
                    Ok(found) => this.found = found,
                    Err(error) => log::info!("could not look for skills: {error}"),
                }
                cx.notify();
            });
        });
        self._tasks.retain(|task| !task.is_ready());
        self._tasks.push(task);
    }

    /// Every skill: what pi loaded, the ones that come with the app, project skills pi skipped,
    /// then the rest of the libraries.
    fn entries(&self, cx: &App) -> Vec<Entry> {
        let builtin_dir = &cx.global::<AppConfig>().builtin_skills_dir;
        let session = self.session.read(cx);
        let here = self.session_project(cx);
        let mut entries: Vec<Entry> = session
            .commands()
            .iter()
            .filter(|command| command.source == "skill")
            .map(|command| {
                let info = command.source_info.clone().unwrap_or_default();
                let builtin = builtin_skills::is_builtin(builtin_dir, &info.path);
                let scope = match info.scope.as_str() {
                    _ if builtin => Scope::BuiltIn,
                    "project" => Scope::Project,
                    "user" => Scope::Personal,
                    _ => Scope::Other,
                };
                Entry {
                    name: command
                        .name
                        .strip_prefix("skill:")
                        .unwrap_or(&command.name)
                        .to_owned(),
                    description: command.description.clone().unwrap_or_default(),
                    project: (scope == Scope::Project).then(|| here.clone()).flatten(),
                    scope,
                    from_package: info.origin == "package" || info.path.contains("/node_modules/"),
                    on_request: self.on_request.contains(&info.path),
                    path: info.path,
                    loaded: true,
                }
            })
            .collect();
        // pi on a host does not get them; what a running pi here says about them counts.
        if session.host().is_none() && session.lifecycle() != &Lifecycle::Ready {
            for (name, path, text) in builtin_skills::all(builtin_dir) {
                let path = path.to_string_lossy().into_owned();
                if entries.iter().any(|entry| entry.path == path) {
                    continue;
                }
                let front = skills::parse_front_matter(text);
                entries.push(Entry {
                    name: name.to_owned(),
                    description: front.description.unwrap_or_default(),
                    on_request: front.only_on_request,
                    path,
                    scope: Scope::BuiltIn,
                    project: None,
                    loaded: true,
                    from_package: false,
                });
            }
        }
        if let Some(Ok(scan)) = &self.scan {
            for (path, front) in &scan.skills {
                if entries.iter().any(|entry| &entry.path == path) {
                    continue;
                }
                entries.push(Entry {
                    name: name_of(path, front),
                    description: front.description.clone().unwrap_or_default(),
                    path: path.clone(),
                    scope: Scope::Project,
                    project: here.clone(),
                    loaded: false,
                    from_package: false,
                    on_request: front.only_on_request,
                });
            }
        }
        let ready = session.lifecycle() == &Lifecycle::Ready;
        for found in &self.found {
            let name = found.name();
            let known = entries.iter().any(|entry| {
                // A host's own library has another path in the folder pi runs with there.
                entry.path == found.path
                    || (found.project.is_none()
                        && entry.scope == Scope::Personal
                        && entry.name == name)
            });
            if known {
                continue;
            }
            entries.push(Entry {
                name,
                description: found.front.description.clone().unwrap_or_default(),
                path: found.path.clone(),
                scope: if found.project.is_some() {
                    Scope::Project
                } else {
                    Scope::Personal
                },
                project: found.project.clone(),
                // A running pi would have reported the user's own.
                loaded: found.project.is_some() || !ready,
                from_package: false,
                on_request: found.front.only_on_request,
            });
        }
        // The session's project first.
        let order = |entry: &Entry| {
            (
                entry.scope,
                entry.project != here,
                entry.project.clone(),
                entry.name.clone(),
            )
        };
        entries.sort_by_cached_key(order);
        entries
    }

    /// Whether the open skill's text was changed and not saved.
    fn is_changed(&self, cx: &App) -> bool {
        let (Some(Ok(text)), Selection::Skill(path)) = (&self.loaded_text, &self.selection) else {
            return false;
        };
        let entries = self.entries(cx);
        let entry = entries.iter().find(|entry| &entry.path == path);
        let saved = self.writing.as_deref().unwrap_or(text);
        !entry.is_some_and(Entry::is_read_only)
            && self.editor.read(cx).value().replace("\r\n", "\n") != saved.replace("\r\n", "\n")
    }

    /// Puts the open skill's text back as it was.
    fn discard_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Ok(text)) = &self.loaded_text {
            let text = text.clone();
            self.editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
        self.detail_error = None;
        cx.notify();
    }

    /// The form of a new skill, with a template for the instructions.
    fn prepare_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form_error = None;
        if std::mem::take(&mut self.reset_form) {
            for input in [&self.name, &self.description] {
                input.update(cx, |input, cx| input.set_value("", window, cx));
            }
            self.only_on_request = false;
            self.instructions
                .update(cx, |editor, cx| editor.set_value("", window, cx));
        }
        if self.instructions.read(cx).value().trim().is_empty() {
            self.instructions
                .update(cx, |editor, cx| editor.set_value(TEMPLATE, window, cx));
        }
        self.name.update(cx, |input, cx| input.focus(window, cx));
    }

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if selection == self.selection && selection != Selection::None {
            return;
        }
        if self.is_changed(cx) {
            self.detail_error = Some("Save or discard your changes first.".into());
            cx.notify();
            return;
        }
        self.detail_error = None;
        self.confirm_trash = false;
        self.notice = None;
        if let Selection::Skill(path) = &selection {
            self.load_skill(path.clone(), cx);
        }
        if selection == Selection::New {
            self.prepare_form(window, cx);
        }
        self.selection = selection;
        cx.notify();
    }

    fn load_skill(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        self.loaded_text = None;
        self.writing = None;
        // One that comes with the app is written when a session starts, which may be later.
        let dir = cx.global::<AppConfig>().builtin_skills_dir.clone();
        let missing = matches!(place, PiPlace::Local { .. })
            && builtin_skills::is_builtin(&dir, &path)
            && self.session.read(cx).lifecycle() != &Lifecycle::Ready;
        let wanted = Selection::Skill(path.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if missing && let Err(error) = builtin_skills::install(&dir) {
                        return Err(format!("Could not write {path}: {error}"));
                    }
                    place
                        .read(&path)
                        .and_then(|text| text.ok_or_else(|| format!("{path} no longer exists.")))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Another skill was opened meanwhile.
                if this.selection != wanted {
                    return;
                }
                if let Ok(text) = &result {
                    let (editor, text) = (this.editor.clone(), text.clone());
                    // The page's own window, which may not be the active one.
                    let window = this.window;
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| {
                            editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
                        });
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
                        if let Some(text) = this.writing.take() {
                            this.loaded_text = Some(Ok(text));
                        }
                        if let Some(selection) = selection {
                            this.reset_form |= this.selection == Selection::New;
                            this.selection = selection.clone();
                            if let Selection::Skill(path) = &selection {
                                this.load_skill(path.clone(), cx);
                            }
                        }
                        this.confirm_trash = false;
                        this.rescan(cx);
                        cx.emit(SkillsEvent::Changed);
                    }
                    Err(error) => {
                        this.writing = None;
                        on_error(this, error.into());
                    }
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
        if body.trim() == TEMPLATE.trim() {
            self.form_error =
                Some("The instructions are still the template: write what pi should do.".into());
            cx.notify();
            return;
        }
        let Some(place) = &self.place else { return };
        let (path, trust_folder) = match self.new_scope {
            NewScope::Personal => (place.agent_path(&format!("skills/{name}/SKILL.md")), None),
            NewScope::Project => {
                let Some(project) = self.chosen_project(cx) else {
                    self.form_error = Some("Choose the project first.".into());
                    cx.notify();
                    return;
                };
                let trust =
                    (!self.is_trusted(&project) && self.trust_too).then_some(project.clone());
                (format!("{project}/.pi/skills/{name}/SKILL.md"), trust)
            }
        };
        // The list only has what the session's pi loads.
        self.notice = match (self.new_scope, self.chosen_project(cx)) {
            (NewScope::Project, Some(project))
                if Some(&project) != self.session_project(cx).as_ref() =>
            {
                Some(
                    format!(
                        "Created. pi loads it in sessions in {}.",
                        self.describe(&project, cx)
                    )
                    .into(),
                )
            }
            _ => None,
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
                    // pi knows folders by their real path.
                    let real = match place {
                        PiPlace::Local { .. } => std::fs::canonicalize(&folder).ok(),
                        PiPlace::Remote { .. } => None,
                    };
                    let folder = real.map_or(folder, |real| real.to_string_lossy().into_owned());
                    write_trust(place, &folder, Some(true))?;
                }
                Ok(Some(Selection::Skill(path)))
            },
            |this, error| {
                this.notice = None;
                this.form_error = Some(error);
            },
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
        self.writing = Some(text.clone());
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
        self.writing = Some(text.clone());
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

    /// The skills of a library, its projects apart.
    fn render_cards(&self, entries: &[Entry], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let here = self.session_project(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        for group in entries.chunk_by(|a, b| a.scope == b.scope && a.project == b.project) {
            let first = &group[0];
            let heading = match (&first.project, first.scope) {
                (Some(project), _) => {
                    let short = self.describe(project, cx);
                    Some(short.rsplit('/').next().unwrap_or(&short).to_owned())
                }
                (None, Scope::Other) => Some("From settings and packages".to_owned()),
                _ => None,
            };
            if let Some(heading) = heading {
                let mut row = h_flex().px_1().pt_1().gap_1p5().text_xs().text_color(muted);
                if first.project.is_some() {
                    row = row.child(Icon::new(IconName::Folder).xsmall());
                }
                rows.push(row.child(heading).into_any_element());
            }
            if first.project.is_some() && first.project == here {
                rows.extend(self.render_trust(cx));
            }
            rows.extend((group.iter()).map(|entry| self.render_card(entry, cx).into_any_element()));
        }
        rows
    }

    /// One of the three libraries, as a panel.
    fn render_library(
        &self,
        library: Library,
        entries: &[Entry],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let own: Vec<Entry> = (entries.iter())
            .filter(|entry| Library::of(entry.scope) == library)
            .cloned()
            .collect();
        let mut rows = self.render_cards(&own, cx);
        // Trust matters as soon as the project has resources, also without skills.
        if library == Library::Project && own.is_empty() {
            rows.extend(self.render_trust(cx));
        }
        let on_host = self.session.read(cx).host().is_some();
        let (icon, title, detail, nothing) = match library {
            Library::BuiltIn => (
                Icon::from(AppIcon::PiLogo),
                "Comes with Pistachio",
                "Pre-installed, always there",
                if on_host {
                    "Not on this machine"
                } else {
                    "None"
                },
            ),
            Library::Own => (
                Icon::from(AppIcon::Library),
                "Your library",
                "Yours, in every project",
                "No skills of your own yet",
            ),
            Library::Project => (
                Icon::new(IconName::Folder),
                "Project libraries",
                "Only in their own project",
                "No project has skills yet",
            ),
        };
        let empty = own.is_empty();
        v_flex()
            .debug_selector(move || format!("library:{title}"))
            .flex_1()
            .flex_basis(px(0.))
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(border)
            // The π signs show through, faintly; the cards are solid.
            .bg(crate::theme::palette(cx).sidebar.opacity(0.4))
            .child(
                v_flex()
                    .flex_none()
                    .items_center()
                    .gap_0p5()
                    .px_3()
                    .pt_4()
                    .pb_3()
                    .border_b_1()
                    .border_color(border)
                    .child(icon.size_5().text_color(muted))
                    .child(
                        h_flex()
                            .pt_1p5()
                            .gap_1p5()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title)
                            .child(
                                div()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(muted)
                                    .child(own.len().to_string()),
                            ),
                    )
                    .child(div().text_xs().text_color(muted).child(detail)),
            )
            .child(
                v_flex()
                    .id(title)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_2()
                    .p_3()
                    .children(rows)
                    .when(empty, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_center()
                                .text_sm()
                                .text_color(muted.opacity(0.75))
                                .child(nothing),
                        )
                    }),
            )
    }

    /// "+", like the one below the SSH connections.
    fn render_add(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        h_flex().flex_none().w_full().justify_center().child(
            div().debug_selector(|| "add-skill-button".into()).child(
                Button::new("new-skill")
                    .quiet(cx)
                    .icon(IconName::Plus)
                    .accessibility_label("New skill")
                    .tooltip("New skill")
                    .selected(self.selection == Selection::New)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select(Selection::New, window, cx);
                    })),
            ),
        )
    }

    fn render_scan_error(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let Some(Err(error)) = &self.scan else {
            return None;
        };
        let color = crate::theme::palette(cx).error;
        Some(
            div()
                .flex_none()
                .w_full()
                .text_center()
                .text_xs()
                .text_color(color)
                .child(error.clone()),
        )
    }

    /// The page while no skill is open: the libraries beside each other.
    fn render_overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.entries(cx);
        let panels: Vec<AnyElement> = Library::ALL
            .map(|library| {
                self.render_library(library, &entries, cx)
                    .into_any_element()
            })
            .into();
        v_flex()
            .size_full()
            .items_center()
            .gap_4()
            .px_8()
            .pt_8()
            .pb_5()
            .children(self.render_scan_error(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .max_w(px(1100.))
                    .max_h(px(720.))
                    .items_start()
                    .gap_4()
                    .children(panels),
            )
            .child(self.render_add(cx))
            .into_any_element()
    }

    /// The libraries below each other, beside an open skill.
    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.entries(cx);
        let mut list = v_flex().gap_2().pb_3();
        for library in Library::ALL {
            let own: Vec<Entry> = (entries.iter())
                .filter(|entry| Library::of(entry.scope) == library)
                .cloned()
                .collect();
            if own.is_empty() {
                continue;
            }
            let title = match library {
                Library::BuiltIn => "Comes with Pistachio",
                Library::Own => "Your library",
                Library::Project => "Project libraries",
            };
            list = list
                .child(
                    div()
                        .px_1()
                        .pt_2()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .children(self.render_cards(&own, cx));
        }
        v_flex()
            .id("skills-list")
            .size_full()
            .overflow_y_scroll()
            .px_3()
            .pt_2()
            .children(self.render_scan_error(cx))
            .child(list)
            .child(self.render_add(cx))
            .into_any_element()
    }

    /// A skill in the list.
    fn render_card(&self, entry: &Entry, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let colors = crate::theme::palette(cx);
        let selected = self.selection == Selection::Skill(entry.path.clone());
        let hovered = theme.foreground.opacity(0.3);
        let path = entry.path.clone();
        let tag = |text: &'static str, color: Hsla| {
            div()
                .flex_none()
                .px_1p5()
                .rounded_md()
                .bg(color.opacity(0.14))
                .text_xs()
                .text_color(color)
                .child(text)
        };
        v_flex()
            .id(ElementId::Name(format!("skill:{}", entry.path).into()))
            .debug_selector(|| format!("skill:{}", entry.name))
            .flex_none()
            .min_w_0()
            .gap_1p5()
            .p_2p5()
            .rounded_lg()
            .border_1()
            .cursor_pointer()
            .map(|this| {
                if selected {
                    this.bg(theme.accent)
                        .border_color(theme.foreground.opacity(0.55))
                } else {
                    this.bg(theme.background.blend(theme.secondary.opacity(0.45)))
                        .border_color(theme.border)
                        .hover(move |style| style.border_color(hovered))
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(Selection::Skill(path.clone()), window, cx);
            }))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(look::tile(&entry.name, entry.builtin(), cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .truncate()
                            .child(entry.name.clone()),
                    )
                    .when(entry.on_request, |this| {
                        this.child(tag("on request", theme.muted_foreground))
                    })
                    .when(!entry.loaded, |this| {
                        this.child(tag("not loaded", colors.warning))
                    }),
            )
            .when(!entry.description.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .line_clamp(2)
                        .text_ellipsis()
                        .child(entry.description.clone()),
                )
            })
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
                    h_flex().child(
                        Button::new("trust")
                            .small()
                            .label("Trust this project")
                            .on_click(cx.listener(|this, _, _, cx| this.set_trust(true, cx))),
                    ),
                )
        };
        Some(
            div()
                .flex_none()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(cx.theme().background)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let error_color = crate::theme::palette(cx).error;
        let untrusted = self
            .chosen_project(cx)
            .is_some_and(|project| !self.is_trusted(&project));
        let label = |text: &'static str| div().text_xs().text_color(muted).child(text);
        let (only, trust_too) = (self.only_on_request, self.trust_too);
        v_flex()
            .min_h_full()
            .gap_3()
            .child(
                h_flex()
                    .child(
                        div()
                            .flex_1()
                            .text_lg()
                            .font_weight(FontWeight::MEDIUM)
                            .child("New skill"),
                    )
                    .child(self.render_close(cx)),
            )
            .child(div().text_sm().text_color(muted).child(
                "A skill gives pi instructions for one kind of task. pi sees only its name and \
                 description, and reads the instructions when a task matches (or when you type \
                 /skill:name).",
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Name"))
                    .child(Input::new(&self.name))
                    .child(div().text_xs().text_color(muted).child(
                        "Lowercase letters, digits and hyphens, at most 64 characters.",
                    )),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Description: what it does and when to use it"))
                    .child(Input::new(&self.description)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Where it can be used"))
                    .child(self.render_scope(cx))
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
                    .flex_1()
                    .gap_1()
                    .child(label("Instructions (Markdown): a template to write over"))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(260.))
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .overflow_hidden()
                            .relative()
                            // Over the box: a height that comes from the form's is not known
                            // to the editor.
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .child(Editor::new(&self.instructions).size_full()),
                            ),
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
                footer(self.form_error.clone().map(|error| (error, error_color)))
                    .child(buttons().child(
                        Button::new("cancel-skill")
                            .form_other(cx)
                            .icon(IconName::Close)
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select(Selection::None, window, cx);
                            })),
                    )
                    .child(
                        Button::new("create-skill")
                            .form_action(cx)
                            .icon(IconName::Check)
                            .label(if self.busy { "Creating…" } else { "Create skill" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                    )),
            )
            .into_any_element()
    }

    /// Where a new skill can be used: everywhere, or in a project chosen from a menu.
    fn render_scope(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let face = |icon: Icon, title: &'static str, detail: SharedString, menu: bool| {
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2p5()
                .child(icon.text_color(muted))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .items_start()
                        .child(title)
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(muted)
                                .truncate()
                                .child(detail),
                        ),
                )
                .when(menu, |this| {
                    this.child(Icon::new(IconName::ChevronDown).small().text_color(muted))
                })
        };
        let for_project = self.new_scope == NewScope::Project;
        let chosen = self.chosen_project(cx);
        let detail = chosen.as_deref().map_or_else(
            || "Choose a project".into(),
            |project| format!("Only in {}", self.describe(project, cx)).into(),
        );
        let projects: Vec<(String, SharedString, SharedString)> = self
            .projects(cx)
            .into_iter()
            .map(|project| {
                let short = self.describe(&project, cx);
                let (parent, name) = match short.rsplit_once('/') {
                    Some((parent, name)) if !name.is_empty() => {
                        (parent.to_owned(), name.to_owned())
                    }
                    _ => (String::new(), short),
                };
                (project, name.into(), parent.into())
            })
            .collect();
        let local = self.session.read(cx).host().is_none();
        let page = cx.weak_entity();
        // A built menu is kept by its button's id, so the id says what is in it.
        let key = (&chosen, for_project, &projects);
        let project_button = Button::new(menu_id("scope-project", &key))
            .choice_card(for_project, cx)
            .child(face(
                Icon::new(IconName::Folder),
                "One project",
                detail,
                true,
            ))
            .accessibility_label("One project")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let checked = for_project.then(|| chosen.clone()).flatten();
                project_menu(menu, &projects, checked.as_deref(), local, &page)
            });
        h_flex()
            .w_full()
            .gap_2()
            .child(
                div().flex_1().flex_basis(px(0.)).min_w_0().child(
                    Button::new("scope-personal")
                        .choice_card(!for_project, cx)
                        .child(face(
                            Icon::from(AppIcon::Globe),
                            "All projects",
                            "pi can use it everywhere".into(),
                            false,
                        ))
                        .accessibility_label("All projects")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.new_scope = NewScope::Personal;
                            this.form_error = None;
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .debug_selector(|| "scope-project".into())
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w_0()
                    .child(project_button),
            )
    }

    /// Closes what is open beside the list, which takes the space again.
    fn render_close(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        Button::new("close-detail")
            .quiet(cx)
            .small()
            .icon(IconName::Close)
            .accessibility_label("Close")
            .tooltip("Close")
            .on_click(cx.listener(|this, _, window, cx| {
                this.select(Selection::None, window, cx);
            }))
    }

    fn render_skill(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let colors = crate::theme::palette(cx);
        let entry = self
            .entries(cx)
            .into_iter()
            .find(|entry| entry.path == path);
        let home = cx.global::<AppConfig>().home.clone();
        let shown_path = self.place.as_ref().map_or_else(
            || path.to_owned(),
            |place| place.describe(path, home.as_deref()),
        );
        let message = |text: SharedString, color: Hsla, close: AnyElement| {
            h_flex()
                .gap_2()
                .child(div().flex_1().text_sm().text_color(color).child(text))
                .child(close)
                .into_any_element()
        };
        let text = match &self.loaded_text {
            None => {
                let close = self.render_close(cx).into_any_element();
                return message("Reading SKILL.md…".into(), muted, close);
            }
            Some(Err(error)) => {
                let close = self.render_close(cx).into_any_element();
                return message(error.clone(), colors.error, close);
            }
            Some(Ok(text)) => text,
        };
        let front = skills::parse_front_matter(text);
        let name = entry
            .as_ref()
            .map(|entry| entry.name.clone())
            .or(front.name.clone())
            .unwrap_or_default();
        let read_only = entry.as_ref().is_some_and(Entry::is_read_only);
        let builtin = entry.as_ref().is_some_and(|entry| entry.builtin());
        let loaded = entry.as_ref().is_none_or(|entry| entry.loaded);
        let only = skills::parse_front_matter(&self.editor.read(cx).value()).only_on_request;
        let (toggle_path, save_path, trash_path) =
            (path.to_owned(), path.to_owned(), path.to_owned());
        v_flex()
            .h_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(look::tile(&name, builtin, cx))
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
                    )
                    .child(self.render_close(cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(shown_path),
            )
            .when(!loaded, |this| {
                let reason = if entry.as_ref().is_some_and(|e| e.scope == Scope::Project) {
                    "pi does not load this skill: the project is not trusted (see the left)."
                } else {
                    "pi did not load this skill: its description may be missing, or another \
                     skill may have its name."
                };
                this.child(div().text_xs().text_color(colors.warning).child(reason))
            })
            .when(read_only, |this| {
                this.child(div().text_xs().text_color(muted).child(if builtin {
                    "This skill comes with Pistachio, which restores it whenever a session \
                     starts. To change it, copy it into a new skill with another name."
                } else {
                    "This skill comes from a pi package; change it where the package comes from."
                }))
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
                    .flex_1()
                    .min_h(px(120.))
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(Editor::new(&self.editor).size_full()),
            )
            .when(!read_only, |this| {
                this.child(if self.confirm_trash {
                    self.render_confirm_trash(&name, trash_path, cx)
                } else {
                    self.render_skill_footer(save_path, cx)
                })
            })
            .into_any_element()
    }

    /// Below an open skill: what it is like now, and what can be done with it.
    fn render_skill_footer(&self, path: String, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let colors = crate::theme::palette(cx);
        let changed = self.is_changed(cx);
        let (status, color) = match (&self.detail_error, &self.notice) {
            (Some(error), _) => (Some(error.clone()), colors.error),
            (None, _) if changed => (Some("Unsaved changes".into()), muted),
            (None, notice) => (notice.clone(), muted),
        };
        footer(status.map(|status| (status, color)))
            .child(
                buttons()
                    .child(
                        Button::new("trash-skill")
                            .form_danger(cx)
                            .icon(Icon::from(AppIcon::Trash))
                            .label("Move to trash")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_trash = true;
                                cx.notify();
                            })),
                    )
                    .when(changed, |this| {
                        this.child(
                            Button::new("discard-skill")
                                .form_other(cx)
                                .icon(Icon::from(AppIcon::RotateCcw))
                                .label("Discard")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.discard_changes(window, cx);
                                })),
                        )
                    })
                    .child(
                        // Green once there is something to save.
                        Button::new("save-skill")
                            .map(|button| {
                                if changed {
                                    button.form_action(cx)
                                } else {
                                    button.form_other(cx)
                                }
                            })
                            .icon(IconName::Check)
                            .label(if self.busy { "Saving…" } else { "Save" })
                            .disabled(self.busy || !changed)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_skill(path.clone(), cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_confirm_trash(&self, name: &str, path: String, cx: &mut Context<Self>) -> AnyElement {
        let question = format!("Move {name} to the trash?").into();
        footer(Some((question, cx.theme().foreground)))
            .child(
                buttons()
                    .child(
                        Button::new("trash-no")
                            .form_other(cx)
                            .icon(IconName::Close)
                            .label("Keep it")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_trash = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("trash-yes")
                            .form_danger(cx)
                            .icon(Icon::from(AppIcon::Trash))
                            .label("Move to trash")
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.trash_skill(path.clone(), cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SkillsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("skills");
        let detail = match self.selection.clone() {
            Selection::None => None,
            Selection::New => Some(self.render_new(cx)),
            Selection::Skill(path) => Some(self.render_skill(&path, cx)),
        };
        // The skills have the space; opening one (or a new one) gives it to that.
        let open = spring(
            ElementId::from("skills-open"),
            if detail.is_some() { 1. } else { 0. },
            cx.theme().motion_tokens().spring_move,
            window,
            cx,
        )
        .clamp(0., 1.);
        let border = cx.theme().border;
        div()
            .relative()
            .flex()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            // The field is its own cached view, as on the welcome screen.
            .when(detail.is_none(), |this| {
                this.child(
                    div().absolute().top_0().left_0().size_full().child(
                        self.pi_field
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
            })
            .child(
                div()
                    .relative()
                    .debug_selector(|| "skills-panel".into())
                    .flex_basis(LIST_WIDTH)
                    .flex_grow(1. - open)
                    .min_w_0()
                    .h_full()
                    .when(open > 0., |this| this.border_r_1().border_color(border))
                    .child(if detail.is_some() {
                        self.render_list(cx)
                    } else {
                        self.render_overview(cx)
                    }),
            )
            .child(
                div()
                    .id("skill-detail")
                    .relative()
                    .debug_selector(|| "skill-detail".into())
                    .flex_basis(px(0.))
                    .flex_grow(open)
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .when_some(detail, |this, detail| {
                        this.overflow_y_scroll().child(
                            div()
                                .size_full()
                                // Not squeezed while it opens.
                                .min_w(px(380.))
                                .max_w(px(920.))
                                .mx_auto()
                                .p_5()
                                .child(detail),
                        )
                    }),
            )
    }
}

/// Below a form: what there is to say, over the buttons.
fn footer(status: Option<(SharedString, Hsla)>) -> Div {
    v_flex()
        .flex_none()
        .w_full()
        .gap_2()
        .pt_1()
        .when_some(status, |this, (text, color)| {
            this.child(
                div()
                    .w_full()
                    .text_center()
                    .text_sm()
                    .text_color(color)
                    .child(text),
            )
        })
}

/// The projects a new skill can be for.
fn project_menu(
    menu: PopupMenu,
    projects: &[(String, SharedString, SharedString)],
    checked: Option<&str>,
    local: bool,
    page: &WeakEntity<SkillsView>,
) -> PopupMenu {
    let mut menu = menu
        .min_w(px(280.))
        .max_h(px(340.))
        .scrollable(true)
        .label("Project");
    for (project, name, parent) in projects {
        // Looked at when the menu opens, not before: looking at a folder under Desktop or
        // Documents can make macOS ask for access.
        if local && !std::path::Path::new(project).is_dir() {
            continue;
        }
        let (page, chosen) = (page.clone(), project.clone());
        let (name, parent) = (name.clone(), parent.clone());
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| {
                let muted = cx.theme().muted_foreground;
                h_flex()
                    .debug_selector(|| format!("project:{name}"))
                    .max_w(px(380.))
                    .min_w_0()
                    .gap_2()
                    .child(Icon::new(IconName::Folder).small().text_color(muted))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(220.))
                            .truncate()
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(parent.clone()),
                    )
            })
            .checked(checked == Some(project.as_str()))
            .on_click(move |_, _, cx| {
                let chosen = chosen.clone();
                let _ = page.update(cx, |page, cx| page.choose_project(chosen, cx));
            }),
        );
    }
    if !local {
        return menu;
    }
    let page = page.clone();
    if !projects.is_empty() {
        menu = menu.separator();
    }
    menu.item(
        PopupMenuItem::new("Choose a folder…")
            .icon(IconName::FolderOpen)
            .on_click(move |_, _, cx| {
                let _ = page.update(cx, |page, cx| page.pick_project(cx));
            }),
    )
}

/// An id for a dropdown whose menu depends on `key`.
fn menu_id(name: &'static str, key: &impl std::hash::Hash) -> ElementId {
    use std::hash::BuildHasher as _;

    ElementId::NamedInteger(
        name.into(),
        std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
            .hash_one(key),
    )
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

fn name_of(path: &str, front: &FrontMatter) -> String {
    let folder = (path.trim_end_matches("/SKILL.md").rsplit('/').next()).unwrap_or_default();
    front.name.clone().unwrap_or_else(|| folder.to_owned())
}

/// The skills in the user's own library and in `projects` (see [`LIBRARY_SCAN`]).
fn find_skills(
    place: &PiPlace,
    home: Option<&str>,
    projects: &[String],
) -> Result<Vec<Found>, String> {
    let agents = home.map_or_else(String::new, |home| format!("{home}/.agents/skills"));
    let own = place.agent_path("skills");
    let args: Vec<&str> = [own.as_str(), agents.as_str()]
        .into_iter()
        .chain(projects.iter().map(String::as_str))
        .collect();
    Ok(parse_found(&place.run_script(LIBRARY_SCAN, &args, None)?))
}

fn parse_found(stdout: &[u8]) -> Vec<Found> {
    let fields: Vec<String> = stdout
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    let mut found: Vec<Found> = Vec::new();
    for [kind, project, path, head] in fields.as_chunks::<4>().0 {
        if kind == "SKILL" && !found.iter().any(|known| known.path == *path) {
            found.push(Found {
                project: Some(project.clone()).filter(|project| !project.is_empty()),
                path: path.clone(),
                front: skills::parse_front_matter(head),
            });
        }
    }
    found
}

/// Which of the skill files at `paths` pi only loads on request.
fn only_on_request(place: &PiPlace, paths: Vec<String>) -> Result<HashSet<String>, String> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let args: Vec<&str> = paths.iter().map(String::as_str).collect();
    let stdout = place.run_script(READ_HEADS, &args, None)?;
    let heads = stdout.split(|&byte| byte == 0);
    Ok((paths.into_iter().zip(heads))
        .filter(|(_, head)| {
            skills::parse_front_matter(&String::from_utf8_lossy(head)).only_on_request
        })
        .map(|(path, _)| path)
        .collect())
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
    use super::{
        PROJECT_SCAN, PiPlace, Scope, Selection, SkillsView, TEMPLATE, find_skills,
        only_on_request, parse_scan, scan_project, write_trust,
    };
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

    #[test]
    fn skills_only_loaded_on_request_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, on_request: bool| {
            let path = dir.path().join(name);
            let text = pi_config::skills::render_skill(name, "Does it.", on_request, "Body.");
            fs::write(&path, text).unwrap();
            path.to_str().unwrap().to_owned()
        };
        let (by_itself, asked) = (file("by-itself", false), file("it's asked for", true));
        let missing = dir.path().join("missing").to_str().unwrap().to_owned();
        let place = PiPlace::Local {
            agent_dir: dir.path().into(),
        };
        let found = only_on_request(&place, vec![by_itself, missing, asked.clone()]).unwrap();
        assert_eq!(found.into_iter().collect::<Vec<_>>(), [asked]);
        assert!(only_on_request(&place, Vec::new()).unwrap().is_empty());
    }

    /// On the welcome screen no pi reports any skills; those that come with the app are listed
    /// all the same, and can be read. On a host they are not: pi there does not get them.
    #[gpui_kit::test]
    fn built_in_skills_are_listed_before_pi_runs(cx: &mut gpui_kit::TestAppContext) {
        use crate::chat::session::PiSession;
        use crate::config::AppConfig;
        use gpui_kit::AppContext as _;

        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skills/add-skill/SKILL.md");
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            let mut config = AppConfig::for_tests("pi".into());
            config.builtin_skills_dir = dir.path().join("skills");
            cx.set_global(config);
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
        });
        let project = dir.path().to_path_buf();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|_| PiSession::idle(project, pi_sessions::Mode::Code));
            SkillsView::new(session, window, cx)
        });
        let entries = cx.read(|cx| view.read(cx).entries(cx));
        let shown: Vec<_> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.scope, entry.loaded))
            .collect();
        assert_eq!(shown, [("add-skill", Scope::BuiltIn, true)]);
        assert!(entries[0].description.starts_with("Create a new pi skill"));
        assert!(!entries[0].on_request);

        // Its card opens it, though no session has written it yet.
        assert!(!path.exists());
        view.update(cx, |view, _| {
            view.place = Some(PiPlace::Local {
                agent_dir: dir.path().into(),
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let card = cx.debug_bounds("skill:add-skill").unwrap();
        assert!(
            card.size.height > gpui_kit::px(60.),
            "two lines of description"
        );
        cx.simulate_click(card.center(), gpui_kit::Modifiers::default());
        let selected = cx.read(|cx| view.read(cx).selection.clone());
        assert_eq!(selected, Selection::Skill(entries[0].path.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while cx.read(|cx| view.read(cx).loaded_text.is_none()) {
            assert!(std::time::Instant::now() < deadline, "it was not read");
            std::thread::sleep(std::time::Duration::from_millis(10));
            cx.run_until_parked();
        }
        let text = cx.read(|cx| view.read(cx).loaded_text.clone()).unwrap();
        assert!(text.unwrap().contains("# Adding a skill"));
        assert!(path.exists());

        let host = remote::HostId("box".into());
        let on_host = cx.new(|_| PiSession::idle_on_host(host));
        view.update(cx, |view, cx| view.set_session(on_host, cx));
        assert_eq!(cx.read(|cx| view.read(cx).entries(cx)), []);
    }

    /// The page in a window of 1100px, over a session without a pi.
    fn page<'a>(
        dir: &std::path::Path,
        cx: &'a mut gpui_kit::TestAppContext,
    ) -> (
        gpui_kit::Entity<SkillsView>,
        &'a mut gpui_kit::VisualTestContext,
    ) {
        use gpui_kit::AppContext as _;
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
            let mut config = crate::config::AppConfig::for_tests("pi".into());
            config.builtin_skills_dir = dir.join("built-in");
            cx.set_global(config);
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
        });
        let project = dir.to_path_buf();
        let mut page = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session =
                cx.new(|_| crate::chat::session::PiSession::idle(project, pi_sessions::Mode::Code));
            let view = cx.new(|cx| SkillsView::new(session, window, cx));
            page = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let page = page.unwrap();
        page.update(cx, |page, _| {
            page.place = Some(PiPlace::Local {
                agent_dir: dir.into(),
            });
        });
        page.update(cx, |page, cx| page.rescan(cx));
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(700.)));
        settle(cx);
        (page, cx)
    }

    /// Draws until nothing changes (the panels' widths, then how many cards fit).
    fn settle(cx: &mut gpui_kit::VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
    }

    fn select(
        page: &gpui_kit::Entity<SkillsView>,
        selection: Selection,
        cx: &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(|window, cx| {
            page.update(cx, |page, cx| page.select(selection, window, cx));
        });
        settle(cx);
    }

    /// Opens the skill file at `path` and waits until it is shown.
    fn open(
        page: &gpui_kit::Entity<SkillsView>,
        path: &std::path::Path,
        cx: &mut gpui_kit::VisualTestContext,
    ) {
        select(page, Selection::Skill(path.to_str().unwrap().into()), cx);
        wait_for(cx, |cx| page.read(cx).loaded_text.is_some());
    }

    fn wait_for(cx: &mut gpui_kit::VisualTestContext, done: impl Fn(&gpui_kit::App) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !cx.read(&done) {
            assert!(std::time::Instant::now() < deadline, "nothing happened");
            std::thread::sleep(std::time::Duration::from_millis(10));
            cx.run_until_parked();
        }
        settle(cx);
    }

    /// The skills have the whole page until one is opened or added, which then gets most of it.
    #[gpui_kit::test]
    fn what_is_open_gets_the_space(cx: &mut gpui_kit::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (page, cx) = page(dir.path(), cx);
        let widths = |cx: &mut gpui_kit::VisualTestContext| {
            ["skills-panel", "skill-detail"]
                .map(|name| f32::from(cx.debug_bounds(name).unwrap().size.width))
        };
        let [list, detail] = widths(cx);
        assert!(list > 1090. && detail < 1., "{list} {detail}");

        // Three equal libraries, and "+" centered below them.
        let panels = ["Comes with Pistachio", "Your library", "Project libraries"].map(|title| {
            cx.debug_bounds(Box::leak(format!("library:{title}").into_boxed_str()))
                .unwrap()
        });
        assert!(panels.windows(2).all(|pair| {
            pair[0].right() < pair[1].left()
                && (pair[0].size.width - pair[1].size.width).abs() < gpui_kit::px(1.)
                && pair[0].size.height == pair[1].size.height
        }));
        let add = cx.debug_bounds("add-skill-button").unwrap();
        assert!(add.top() >= panels[1].bottom());
        assert!((add.center().x - panels[1].center().x).abs() < gpui_kit::px(1.));
        // The one that comes with the app is in the first.
        let card = cx.debug_bounds("skill:add-skill").unwrap();
        assert!(panels[0].contains(&card.center()));
        cx.simulate_click(add.center(), gpui_kit::Modifiers::default());
        settle(cx);
        assert_eq!(
            cx.read(|cx| page.read(cx).selection.clone()),
            Selection::New
        );
        let [list, detail] = widths(cx);
        assert!(list < 300. && detail > 800., "{list} {detail}");

        select(&page, Selection::None, cx);
        let [list, detail] = widths(cx);
        assert!(list > 1090. && detail < 1., "{list} {detail}");
    }

    /// A new skill is for every project, or for one that is chosen; the session's comes first.
    #[gpui_kit::test]
    fn a_new_skill_can_be_for_a_chosen_project(cx: &mut gpui_kit::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        let (page, cx) = page(&root, cx);
        cx.update(|_, cx| {
            cx.set_global(crate::config::RecentFolders(vec![project.clone()]));
        });
        let project = project.to_str().unwrap().to_owned();
        select(&page, Selection::New, cx);
        cx.read(|cx| {
            let page = page.read(cx);
            // No session is open, so there is no project yet.
            assert_eq!(page.chosen_project(cx), None);
            assert_eq!(page.projects(cx), std::slice::from_ref(&project));
        });
        let create = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                page.update(cx, |page, cx| {
                    for (input, text) in [(&page.name, "my-skill"), (&page.description, "Does it.")]
                    {
                        input.update(cx, |input, cx| input.set_value(text, window, cx));
                    }
                    page.instructions
                        .update(cx, |editor, cx| editor.set_value("Do it.", window, cx));
                    page.create(cx);
                });
            });
        };
        page.update(cx, |page, _| page.new_scope = super::NewScope::Project);
        create(cx);
        let error = cx.read(|cx| page.read(cx).form_error.clone());
        assert!(error.unwrap().contains("Choose the project"));

        // Chosen from the menu of the "One project" card.
        page.update(cx, |page, _| page.new_scope = super::NewScope::Personal);
        settle(cx);
        let card = cx.debug_bounds("scope-project").unwrap();
        cx.simulate_click(card.center(), gpui_kit::Modifiers::default());
        settle(cx);
        let item = cx
            .debug_bounds("project:project")
            .expect("the menu lists it");
        cx.simulate_click(item.center(), gpui_kit::Modifiers::default());
        settle(cx);
        cx.read(|cx| {
            let page = page.read(cx);
            assert_eq!(page.chosen_project(cx).as_deref(), Some(project.as_str()));
            assert_eq!(page.new_scope, super::NewScope::Project);
        });
        create(cx);
        wait_for(cx, |cx| page.read(cx).loaded_text.is_some());
        let file = std::path::Path::new(&project).join(".pi/skills/my-skill/SKILL.md");
        assert!(file.exists());
        // It says where pi loads it, and the project is trusted, as asked.
        let notice = cx.read(|cx| page.read(cx).notice.clone());
        assert!(notice.unwrap().contains("project"));
        let trust = fs::read_to_string(root.join("trust.json")).unwrap();
        assert!(trust.contains(&project), "{trust}");
    }

    /// A new skill starts with a template, which has to be written over, and is kept while the
    /// form is closed.
    #[gpui_kit::test]
    fn a_new_skill_starts_with_a_template(cx: &mut gpui_kit::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (page, cx) = page(dir.path(), cx);
        let set = |text: &str, cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| {
                page.update(cx, |page, cx| {
                    let text = text.to_owned();
                    page.instructions
                        .update(cx, |editor, cx| editor.set_value(text, window, cx));
                });
            });
        };
        let instructions = |cx: &gpui_kit::VisualTestContext| {
            cx.read(|cx| page.read(cx).instructions.read(cx).value())
        };
        select(&page, Selection::New, cx);
        assert_eq!(instructions(cx).trim(), TEMPLATE.trim());
        cx.update(|window, cx| {
            page.update(cx, |page, cx| {
                for (input, text) in [(&page.name, "my-skill"), (&page.description, "Does it.")] {
                    input.update(cx, |input, cx| input.set_value(text, window, cx));
                }
                page.create(cx);
            });
        });
        settle(cx);
        let error = cx.read(|cx| page.read(cx).form_error.clone());
        assert!(error.unwrap().contains("still the template"));

        set("Do the thing.", cx);
        select(&page, Selection::None, cx);
        select(&page, Selection::New, cx);
        assert_eq!(instructions(cx).trim(), "Do the thing.");

        // Created, it opens, and the next one starts over.
        page.update(cx, |page, cx| page.create(cx));
        let file = dir.path().join("skills/my-skill/SKILL.md");
        wait_for(cx, |cx| page.read(cx).loaded_text.is_some());
        assert!(fs::read_to_string(&file).unwrap().contains("Do the thing."));
        let opened = Selection::Skill(file.to_str().unwrap().into());
        assert_eq!(cx.read(|cx| page.read(cx).selection.clone()), opened);
        select(&page, Selection::New, cx);
        assert_eq!(instructions(cx).trim(), TEMPLATE.trim());
        assert!(cx.read(|cx| page.read(cx).name.read(cx).value().is_empty()));
    }

    /// A skill opens with its text whichever window is active. What was changed is not lost by
    /// opening something else, and can be discarded or saved.
    #[gpui_kit::test]
    fn changes_are_kept_until_saved_or_discarded(cx: &mut gpui_kit::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("SKILL.md");
        let text = pi_config::skills::render_skill("one", "Does it.", false, "Body.");
        fs::write(&file, &text).unwrap();
        let (page, cx) = page(dir.path(), cx);
        open(&page, &file, cx);
        let shown =
            |cx: &gpui_kit::VisualTestContext| cx.read(|cx| page.read(cx).editor.read(cx).value());
        let changed = |cx: &gpui_kit::VisualTestContext| cx.read(|cx| page.read(cx).is_changed(cx));
        assert_eq!(shown(cx), text);
        assert!(!changed(cx));

        let edit = |cx: &mut gpui_kit::VisualTestContext| {
            let text = format!("{text}More.\n");
            cx.update(|window, cx| {
                page.update(cx, |page, cx| {
                    page.editor
                        .update(cx, |editor, cx| editor.set_value(text, window, cx));
                });
            });
            settle(cx);
        };
        edit(cx);
        assert!(changed(cx));
        select(&page, Selection::New, cx);
        let here = Selection::Skill(file.to_str().unwrap().into());
        assert_eq!(cx.read(|cx| page.read(cx).selection.clone()), here);
        let error = cx.read(|cx| page.read(cx).detail_error.clone());
        assert!(error.unwrap().contains("Save or discard"));

        cx.update(|window, cx| {
            page.update(cx, |page, cx| page.discard_changes(window, cx));
        });
        settle(cx);
        assert_eq!(shown(cx), text);
        assert!(!changed(cx));

        edit(cx);
        page.update(cx, |page, cx| {
            page.save_skill(file.to_str().unwrap().into(), cx);
        });
        wait_for(cx, |cx| !page.read(cx).busy);
        assert!(fs::read_to_string(&file).unwrap().ends_with("More.\n"));
        assert!(!changed(cx));
        select(&page, Selection::None, cx);
        assert_eq!(
            cx.read(|cx| page.read(cx).selection.clone()),
            Selection::None
        );
    }

    /// The libraries are known without pi: the user's own skills, and each project's.
    #[test]
    fn skills_are_found_in_every_library() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let (one, two) = (root.join("one"), root.join("it's two"));
        for (dir, name) in [
            (root.join("agent/skills/notes"), "notes"),
            (home.join(".agents/skills/shared"), "shared"),
            (one.join(".pi/skills/guide"), "guide"),
            (one.join(".agents/skills/more"), "more"),
            (two.join(".pi/skills/plots"), "plots"),
            (two.join(".pi/skills/node_modules/x"), "x"),
        ] {
            fs::create_dir_all(&dir).unwrap();
            let text = pi_config::skills::render_skill(name, "Does it.", name == "plots", "B");
            fs::write(dir.join("SKILL.md"), text).unwrap();
        }
        let place = PiPlace::Local {
            agent_dir: root.join("agent"),
        };
        let text = |path: &std::path::Path| path.to_str().unwrap().to_owned();
        let projects = [
            text(&one),
            text(&two),
            text(&home),
            text(&root.join("gone")),
        ];
        let found = find_skills(&place, home.to_str(), &projects).unwrap();
        let mut shown: Vec<_> = found
            .iter()
            .map(|found| {
                (
                    found.name(),
                    found.project.clone(),
                    found.front.only_on_request,
                )
            })
            .collect();
        shown.sort();
        assert_eq!(
            shown,
            [
                ("guide".into(), Some(text(&one)), false),
                ("more".into(), Some(text(&one)), false),
                ("notes".into(), None, false),
                ("plots".into(), Some(text(&two)), true),
                ("shared".into(), None, false),
            ]
        );
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
        config.child_env = vec![("PI_CODING_AGENT_DIR".into(), agent.path().into())].into();
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
                    .filter(|command| {
                        command.source == "skill" && command.name != "skill:add-skill"
                    })
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

        // The page lists both as loaded, and the one that comes with the app.
        let (view, cx) =
            cx.add_window_view(|window, cx| SkillsView::new(third.clone(), window, cx));
        let entries = cx.read(|cx| view.read(cx).entries(cx));
        let shown: Vec<(&str, bool)> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.loaded))
            .collect();
        assert_eq!(
            shown,
            [
                ("repo-guide", true),
                ("release-notes", true),
                ("add-skill", true)
            ]
        );
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
