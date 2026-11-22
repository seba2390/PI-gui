//! The "Diff" tab's model: the repository status plus the parsed patches of expanded files,
//! flattened into the rows of one virtualized list.
//!
//! Pure and GPUI-free apart from [`SharedString`]. Loading happens elsewhere (on the
//! background executor); this model only tracks which patches are needed and accepts results,
//! rejecting stale ones by token, like the file tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::SharedString;
use workspace::git::{ChangeKind, FileChange, RepoStatus};

use super::diff_lines::ParsedDiff;

/// Colour class of a status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Added,
    Modified,
    Removed,
    Renamed,
    Conflict,
    Other,
}

/// Display data of one changed file, computed once per status load (off the UI thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub change: FileChange,
    /// File name.
    pub name: SharedString,
    /// Dim text after the name: the parent directory, or where a rename came from.
    pub detail: SharedString,
    /// Status badge, e.g. `M`.
    pub letter: SharedString,
    pub tone: Tone,
    /// `+N`, when lines were added.
    pub additions: Option<SharedString>,
    /// `−N`, when lines were removed.
    pub deletions: Option<SharedString>,
}

impl FileEntry {
    pub fn new(change: FileChange) -> Self {
        let name = change.path.file_name().map_or_else(
            || change.path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let detail = match &change.kind {
            ChangeKind::Renamed { from } => format!("from {}", from.display()),
            _ => change
                .path
                .parent()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let (letter, tone) = badge(&change.kind);
        let count =
            |sign: &str, n: Option<u32>| n.filter(|n| *n > 0).map(|n| format!("{sign}{n}").into());
        Self {
            name: name.into(),
            detail: detail.into(),
            letter: letter.into(),
            tone,
            additions: count("+", change.additions),
            deletions: count("\u{2212}", change.deletions),
            change,
        }
    }

    pub fn path(&self) -> &Path {
        &self.change.path
    }
}

/// Badge letter and tone. Untracked files read `U` and conflicts `!`, as in VS Code, rather
/// than git's `?` and `U`.
fn badge(kind: &ChangeKind) -> (&'static str, Tone) {
    match kind {
        ChangeKind::Added => ("A", Tone::Added),
        ChangeKind::Untracked => ("U", Tone::Added),
        ChangeKind::Modified => ("M", Tone::Modified),
        ChangeKind::Deleted => ("D", Tone::Removed),
        ChangeKind::Renamed { .. } => ("R", Tone::Renamed),
        ChangeKind::Conflicted => ("!", Tone::Conflict),
        ChangeKind::TypeChanged => ("T", Tone::Other),
    }
}

/// The header summary, e.g. `main · 3 files  +42 −7`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// `main · 3 files`.
    pub label: SharedString,
    pub additions: Option<SharedString>,
    pub deletions: Option<SharedString>,
}

impl Summary {
    pub fn new(repo: &RepoStatus) -> Self {
        let branch = repo
            .branch
            .as_deref()
            .or(repo.head_short.as_deref())
            .unwrap_or("HEAD");
        let files = match repo.files.len() {
            0 => "no changes".to_owned(),
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        let (additions, deletions) = repo.totals();
        Self {
            label: format!("{branch} \u{b7} {files}").into(),
            additions: (additions > 0).then(|| format!("+{additions}").into()),
            deletions: (deletions > 0).then(|| format!("\u{2212}{deletions}").into()),
        }
    }
}

/// A loaded status, ready to install with [`DiffModel::set_status`].
#[derive(Debug)]
pub struct LoadedStatus {
    pub repo: Arc<RepoStatus>,
    pub files: Vec<FileEntry>,
    pub summary: Summary,
}

impl LoadedStatus {
    pub fn new(repo: RepoStatus) -> Self {
        let summary = Summary::new(&repo);
        let files = repo.files.iter().cloned().map(FileEntry::new).collect();
        Self {
            repo: Arc::new(repo),
            files,
            summary,
        }
    }
}

/// Result of a status load: `Ok(None)` when the folder is not in a git work tree.
pub type StatusResult = Result<Option<LoadedStatus>, String>;

/// What the list shows instead of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Loading,
    NotRepo,
    Failed(SharedString),
    Ready,
}

#[derive(Debug)]
enum Patch {
    Loading,
    Loaded(Rc<ParsedDiff>),
    Failed(SharedString),
}

#[derive(Debug)]
struct Expanded {
    patch: Patch,
    /// Token of the load in flight, if any. Results with another token are stale.
    pending: Option<u64>,
}

/// An annotation row below a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Loading,
    Binary,
    /// The patch has no hunks (e.g. a mode change only).
    NoText,
    /// Lines cut off by [`super::diff_lines::MAX_LINES`].
    Hidden(usize),
    Error(SharedString),
}

/// One row of the flattened list.
#[derive(Debug, Clone)]
pub enum DiffRow {
    File {
        entry: Rc<FileEntry>,
        expanded: bool,
        /// Follows an expanded diff, so it gets a separator line.
        separated: bool,
    },
    Line {
        diff: Rc<ParsedDiff>,
        ix: usize,
    },
    Note(Note),
}

/// Everything needed to load one file's patch.
#[derive(Debug)]
pub struct PatchRequest {
    pub token: u64,
    pub repo: Arc<RepoStatus>,
    pub change: FileChange,
}

#[derive(Debug)]
pub struct DiffModel {
    phase: Phase,
    repo: Option<Arc<RepoStatus>>,
    files: Vec<Rc<FileEntry>>,
    summary: Option<Summary>,
    /// Expanded files by path relative to the repository root.
    expanded: HashMap<PathBuf, Expanded>,
    rows: Vec<DiffRow>,
    next_token: u64,
}

impl Default for DiffModel {
    fn default() -> Self {
        Self {
            phase: Phase::Loading,
            repo: None,
            files: Vec::new(),
            summary: None,
            expanded: HashMap::new(),
            rows: Vec::new(),
            next_token: 0,
        }
    }
}

impl DiffModel {
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn rows(&self) -> &[DiffRow] {
        &self.rows
    }

    pub fn summary(&self) -> Option<&Summary> {
        self.summary.as_ref()
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains_key(path)
    }

    /// Every changed file, in list order.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    }

    /// The expanded files, sorted.
    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.expanded.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// Installs a status. Expanded files that are no longer changed collapse; the others keep
    /// showing their old patch and are returned so the caller reloads them.
    pub fn set_status(&mut self, result: StatusResult) -> Vec<PathBuf> {
        match result {
            Ok(Some(loaded)) => {
                self.phase = Phase::Ready;
                self.files = loaded.files.into_iter().map(Rc::new).collect();
                self.repo = Some(loaded.repo);
                self.summary = Some(loaded.summary);
            }
            Ok(None) => self.clear(Phase::NotRepo),
            Err(error) => self.clear(Phase::Failed(error.into())),
        }
        let files = &self.files;
        self.expanded
            .retain(|path, _| files.iter().any(|file| file.path() == path));
        self.rebuild();
        let mut paths: Vec<PathBuf> = self.expanded.keys().cloned().collect();
        paths.sort();
        paths
    }

    fn clear(&mut self, phase: Phase) {
        self.phase = phase;
        self.files.clear();
        self.repo = None;
        self.summary = None;
    }

    /// Expands or collapses the file at `path`. Returns whether it is now expanded (and so
    /// needs its patch loaded).
    pub fn toggle(&mut self, path: &Path) -> bool {
        let expanded = if self.expanded.remove(path).is_some() {
            false
        } else {
            self.expanded.insert(
                path.to_path_buf(),
                Expanded {
                    patch: Patch::Loading,
                    pending: None,
                },
            );
            true
        };
        self.rebuild();
        expanded
    }

    /// Marks the expanded file at `path` as loading. `None` when it is not expanded (or no
    /// status is loaded). An existing patch stays visible until the result arrives.
    pub fn begin_load(&mut self, path: &Path) -> Option<PatchRequest> {
        let repo = self.repo.clone()?;
        let change = self
            .files
            .iter()
            .find(|file| file.path() == path)?
            .change
            .clone();
        let state = self.expanded.get_mut(path)?;
        self.next_token += 1;
        state.pending = Some(self.next_token);
        Some(PatchRequest {
            token: self.next_token,
            repo,
            change,
        })
    }

    /// Installs a patch. Returns whether the rows changed: false for a stale result, and for
    /// a reload identical to the patch already shown.
    pub fn finish_load(
        &mut self,
        path: &Path,
        token: u64,
        result: Result<ParsedDiff, String>,
    ) -> bool {
        let Some(state) = self.expanded.get_mut(path) else {
            return false;
        };
        if state.pending != Some(token) {
            return false;
        }
        state.pending = None;
        state.patch = match result {
            Ok(diff) if matches!(&state.patch, Patch::Loaded(old) if **old == diff) => {
                return false;
            }
            Ok(diff) => Patch::Loaded(Rc::new(diff)),
            Err(error) => Patch::Failed(error.into()),
        };
        self.rebuild();
        true
    }

    /// Recomputes the flattened rows: each file, followed by its patch when expanded.
    fn rebuild(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len().max(self.files.len()));
        let mut separated = false;
        for entry in &self.files {
            let expanded = self.expanded.get(entry.path());
            rows.push(DiffRow::File {
                entry: entry.clone(),
                expanded: expanded.is_some(),
                separated,
            });
            let Some(expanded) = expanded else {
                separated = false;
                continue;
            };
            separated = true;
            match &expanded.patch {
                Patch::Loading => rows.push(DiffRow::Note(Note::Loading)),
                Patch::Failed(error) => rows.push(DiffRow::Note(Note::Error(error.clone()))),
                Patch::Loaded(diff) => {
                    if diff.binary {
                        rows.push(DiffRow::Note(Note::Binary));
                    } else if diff.lines.is_empty() && diff.hidden == 0 {
                        rows.push(DiffRow::Note(Note::NoText));
                    }
                    rows.extend((0..diff.lines.len()).map(|ix| DiffRow::Line {
                        diff: diff.clone(),
                        ix,
                    }));
                    if diff.hidden > 0 {
                        rows.push(DiffRow::Note(Note::Hidden(diff.hidden)));
                    }
                }
            }
        }
        self.rows = rows;
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use workspace::git::{ChangeKind, FileChange, RepoStatus};

    use super::{DiffModel, DiffRow, FileEntry, LoadedStatus, Note, Phase, Summary, Tone};
    use crate::panels::right_panel::diff_lines::ParsedDiff;

    fn change(path: &str, kind: ChangeKind, counts: Option<(u32, u32)>) -> FileChange {
        FileChange {
            path: path.into(),
            kind,
            staged: false,
            unstaged: true,
            additions: counts.map(|c| c.0),
            deletions: counts.map(|c| c.1),
        }
    }

    fn repo(files: Vec<FileChange>) -> RepoStatus {
        RepoStatus {
            root: "/r".into(),
            branch: Some("main".into()),
            head_short: Some("abc1234".into()),
            files,
        }
    }

    fn loaded() -> DiffModel {
        let mut model = DiffModel::default();
        assert_eq!(model.phase(), &Phase::Loading);
        let reload = model.set_status(Ok(Some(LoadedStatus::new(repo(vec![
            change("src/a.rs", ChangeKind::Modified, Some((2, 1))),
            change("b.png", ChangeKind::Added, None),
            change("c.txt", ChangeKind::Untracked, Some((3, 0))),
        ])))));
        assert_eq!(reload, Vec::<PathBuf>::new());
        model
    }

    /// One label per row: file names (`/` when expanded, `|` before when separated), line
    /// texts, and notes.
    fn render(model: &DiffModel) -> Vec<String> {
        model
            .rows()
            .iter()
            .map(|row| match row {
                DiffRow::File {
                    entry,
                    expanded,
                    separated,
                } => format!(
                    "{}{}{}",
                    if *separated { "|" } else { "" },
                    entry.name,
                    if *expanded { "/" } else { "" }
                ),
                DiffRow::Line { diff, ix } => format!("  {}", diff.lines[*ix].text),
                DiffRow::Note(note) => format!("  ({note:?})"),
            })
            .collect()
    }

    fn patch(path: &str) -> ParsedDiff {
        ParsedDiff::parse(
            &format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new\n"),
            100,
        )
    }

    #[test]
    fn file_entries_and_summary() {
        let entry = FileEntry::new(change("src/ui/a.rs", ChangeKind::Modified, Some((4, 0))));
        assert_eq!(
            (
                entry.name.as_ref(),
                entry.detail.as_ref(),
                entry.letter.as_ref(),
                entry.tone
            ),
            ("a.rs", "src/ui", "M", Tone::Modified)
        );
        assert_eq!(entry.additions.as_deref(), Some("+4"));
        assert_eq!(entry.deletions, None);

        let renamed = FileEntry::new(change(
            "b.rs",
            ChangeKind::Renamed {
                from: PathBuf::from("old/b.rs"),
            },
            None,
        ));
        assert_eq!(
            (renamed.detail.as_ref(), renamed.letter.as_ref()),
            ("from old/b.rs", "R")
        );
        assert_eq!(
            FileEntry::new(change("x", ChangeKind::Untracked, None))
                .letter
                .as_ref(),
            "U"
        );

        let summary = Summary::new(&repo(vec![
            change("a", ChangeKind::Modified, Some((40, 7))),
            change("b", ChangeKind::Added, Some((2, 0))),
        ]));
        assert_eq!(summary.label.as_ref(), "main \u{b7} 2 files");
        assert_eq!(summary.additions.as_deref(), Some("+42"));
        assert_eq!(summary.deletions.as_deref(), Some("\u{2212}7"));

        let mut detached = repo(vec![change("a", ChangeKind::Deleted, Some((0, 3)))]);
        detached.branch = None;
        let summary = Summary::new(&detached);
        assert_eq!(summary.label.as_ref(), "abc1234 \u{b7} 1 file");
        assert_eq!(summary.additions, None);
    }

    #[test]
    fn expands_and_flattens() {
        let mut model = loaded();
        assert_eq!(render(&model), ["a.rs", "b.png", "c.txt"]);

        assert!(model.toggle(Path::new("src/a.rs")));
        assert_eq!(render(&model), ["a.rs/", "  (Loading)", "|b.png", "c.txt"]);
        let request = model.begin_load(Path::new("src/a.rs")).unwrap();
        assert_eq!(request.change.path, PathBuf::from("src/a.rs"));
        assert!(model.finish_load(Path::new("src/a.rs"), request.token, Ok(patch("src/a.rs"))));

        model.toggle(Path::new("b.png"));
        let token = model.begin_load(Path::new("b.png")).unwrap().token;
        let binary = ParsedDiff::parse("Binary files /dev/null and b/b.png differ\n", 100);
        model.finish_load(Path::new("b.png"), token, Ok(binary));

        assert_eq!(
            render(&model),
            [
                "a.rs/",
                "  @@ -1 +1 @@",
                "  old",
                "  new",
                "|b.png/",
                "  (Binary)",
                "|c.txt",
            ]
        );

        assert!(!model.toggle(Path::new("src/a.rs")));
        assert_eq!(render(&model), ["a.rs", "b.png/", "  (Binary)", "|c.txt"]);
        assert!(!model.is_expanded(Path::new("src/a.rs")));
    }

    #[test]
    fn notes_for_errors_empty_and_hidden() {
        let mut model = loaded();
        model.toggle(Path::new("src/a.rs"));
        let token = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        model.finish_load(Path::new("src/a.rs"), token, Err("boom".into()));
        assert_eq!(render(&model)[1], "  (Error(\"boom\"))");

        let token = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        model.finish_load(Path::new("src/a.rs"), token, Ok(ParsedDiff::default()));
        assert_eq!(render(&model)[1], "  (NoText)");

        let body = (0..5)
            .map(|i| format!("+{i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let capped = ParsedDiff::parse(&format!("@@ -0,0 +1,5 @@\n{body}"), 3);
        let token = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        model.finish_load(Path::new("src/a.rs"), token, Ok(capped));
        assert_eq!(
            render(&model)[1..5],
            ["  @@ -0,0 +1,5 @@", "  0", "  1", "  (Hidden(3))"]
        );
        assert!(matches!(model.rows()[4], DiffRow::Note(Note::Hidden(3))));
    }

    #[test]
    fn rejects_stale_and_unknown_results() {
        let mut model = loaded();
        assert!(
            model.begin_load(Path::new("src/a.rs")).is_none(),
            "not expanded"
        );
        assert!(model.begin_load(Path::new("missing")).is_none());
        model.toggle(Path::new("src/a.rs"));
        let old = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        let new = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        assert!(!model.finish_load(Path::new("src/a.rs"), old, Ok(patch("a"))));
        assert_eq!(render(&model)[1], "  (Loading)");
        assert!(model.finish_load(Path::new("src/a.rs"), new, Ok(patch("a"))));
        // An identical reload changes nothing.
        let same = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        assert!(!model.finish_load(Path::new("src/a.rs"), same, Ok(patch("a"))));
        assert!(
            !model.finish_load(Path::new("src/a.rs"), same, Ok(patch("b"))),
            "consumed"
        );
        assert!(
            !model.finish_load(Path::new("src/a.rs"), new, Ok(patch("a"))),
            "consumed"
        );

        // Collapsed while loading: the result is dropped.
        let token = model.begin_load(Path::new("src/a.rs")).unwrap().token;
        model.toggle(Path::new("src/a.rs"));
        assert!(!model.finish_load(Path::new("src/a.rs"), token, Ok(patch("a"))));
    }

    #[test]
    fn new_status_keeps_patches_and_collapses_vanished_files() {
        let mut model = loaded();
        for path in ["src/a.rs", "c.txt"] {
            model.toggle(Path::new(path));
            let token = model.begin_load(Path::new(path)).unwrap().token;
            model.finish_load(Path::new(path), token, Ok(patch(path)));
        }
        let reload = model.set_status(Ok(Some(LoadedStatus::new(repo(vec![change(
            "src/a.rs",
            ChangeKind::Modified,
            Some((1, 1)),
        )])))));
        assert_eq!(reload, [PathBuf::from("src/a.rs")]);
        // The old patch stays visible until the reload lands.
        assert_eq!(render(&model), ["a.rs/", "  @@ -1 +1 @@", "  old", "  new"]);
        assert!(!model.is_expanded(Path::new("c.txt")));
        assert_eq!(
            model.summary().unwrap().label.as_ref(),
            "main \u{b7} 1 file"
        );

        assert_eq!(model.set_status(Ok(None)), Vec::<PathBuf>::new());
        assert_eq!(model.phase(), &Phase::NotRepo);
        assert!(model.rows().is_empty());
        assert!(model.summary().is_none());

        model.set_status(Err("git failed".into()));
        assert_eq!(model.phase(), &Phase::Failed("git failed".into()));

        let clean = LoadedStatus::new(repo(Vec::new()));
        assert_eq!(Arc::strong_count(&clean.repo), 1);
        model.set_status(Ok(Some(clean)));
        assert_eq!(model.phase(), &Phase::Ready);
        assert!(model.rows().is_empty());
    }
}
