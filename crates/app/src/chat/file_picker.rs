//! The instant "attach a file" picker: fuzzy search over the working folder's files.
//!
//! macOS's open dialog takes a noticeable moment to appear, so the attach button opens this
//! in-app list instead; "Browse other files…" still falls back to the system dialog. The file
//! index is built on a background thread when the composer is created and refreshed each time
//! the picker opens, so typing only ranks an in-memory list.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::list::{ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, h_flex};
use gpui_kit::*;

/// Files indexed per folder; enough for big repositories while keeping ranking instant.
pub const INDEX_LIMIT: usize = 30_000;
/// Results shown per query.
const MAX_RESULTS: usize = 200;

/// What the user chose in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// A file in the working folder (absolute path).
    File(PathBuf),
    /// Open the system file dialog instead.
    Browse,
    /// Escape: close without attaching.
    Dismiss,
}

type OnPick = Rc<dyn Fn(Pick, &mut Window, &mut App)>;

pub struct FilePicker {
    root: PathBuf,
    /// Where `root` lives (this computer or an SSH host).
    location: workspace::Location,
    files: Arc<Vec<String>>,
    /// Indices into `files`, best match first.
    matches: Vec<usize>,
    query: String,
    loading: bool,
    truncated: bool,
    selected: Option<IndexPath>,
    on_pick: OnPick,
}

impl FilePicker {
    pub fn new(
        root: PathBuf,
        location: workspace::Location,
        on_pick: impl Fn(Pick, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            root,
            location,
            files: Arc::default(),
            matches: Vec::new(),
            query: String::new(),
            loading: true,
            truncated: false,
            selected: None,
            on_pick: Rc::new(on_pick),
        }
    }

    /// Installs a freshly built index (from [`workspace::files::list_files`]).
    pub fn set_files(&mut self, files: Vec<String>, truncated: bool) {
        self.files = Arc::new(files);
        self.truncated = truncated;
        self.loading = false;
        self.matches = rank(&self.files, &self.query, MAX_RESULTS);
    }

    /// The folder's indexed files (relative paths), for inline `@` completion.
    pub fn files(&self) -> Arc<Vec<String>> {
        Arc::clone(&self.files)
    }

    fn pick(&self, pick: Pick, window: &mut Window, cx: &mut App) {
        (self.on_pick)(pick, window, cx);
    }
}

/// Loads the index for `picker`'s folder on a background thread.
pub fn refresh_index(picker: &Entity<ListState<FilePicker>>, cx: &mut App) {
    let root = picker.read(cx).delegate().root.clone();
    let location = picker.read(cx).delegate().location.clone();
    let picker = picker.downgrade();
    cx.spawn(async move |cx| {
        let (files, truncated) = cx
            .background_executor()
            .spawn(async move { workspace::files::list_files_at(&location, &root, INDEX_LIMIT) })
            .await;
        let _ = picker.update(cx, |state, cx| {
            state.delegate_mut().set_files(files, truncated);
            cx.notify();
        });
    })
    .detach();
}

impl ListDelegate for FilePicker {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        query.clone_into(&mut self.query);
        self.matches = rank(&self.files, query, MAX_RESULTS);
        Task::ready(())
    }

    /// Section 0: matching files; section 1: "Browse other files…".
    fn sections_count(&self, _: &App) -> usize {
        2
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        if section == 0 { self.matches.len() } else { 1 }
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let theme = cx.theme();
        let selected = self.selected == Some(ix);
        let item = ListItem::new(ix).py_1().selected(selected);
        if ix.section == 1 {
            let label = if self.truncated {
                "Browse other files… (folder too large to list fully)"
            } else {
                "Browse other files…"
            };
            return Some(
                item.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::FolderOpen).small())
                        .child(label),
                ),
            );
        }
        let path = self.files.get(*self.matches.get(ix.row)?)?;
        let (dir, name) = path
            .rsplit_once('/')
            .map_or(("", path.as_str()), |(dir, name)| (dir, name));
        Some(
            item.child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .text_sm()
                    .child(
                        Icon::new(IconName::File)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.foreground)
                            .child(name.to_owned()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(dir.to_owned()),
                    ),
            ),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let text = if self.loading {
            "Indexing files…"
        } else {
            "No matching files"
        };
        h_flex()
            .size_full()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(text)
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
        cx.notify();
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let pick = match self.selected {
            Some(ix) if ix.section == 1 => Pick::Browse,
            Some(ix) => match self
                .matches
                .get(ix.row)
                .and_then(|&file| self.files.get(file))
            {
                Some(path) => Pick::File(self.root.join(path)),
                None => return,
            },
            None => return,
        };
        self.pick(pick, window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        self.pick(Pick::Dismiss, window, cx);
    }
}

/// Indices of the `limit` best fuzzy matches of `query` in `files`, best first. An empty query
/// lists the shallowest paths first (top-level files are the likeliest picks).
pub fn rank(files: &[String], query: &str, limit: usize) -> Vec<usize> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let mut scored: Vec<(i64, usize)> = if query.is_empty() {
        files
            .iter()
            .enumerate()
            .map(|(ix, path)| {
                (
                    -(cost(path.matches('/').count()) * 1000 + cost(path.len())),
                    ix,
                )
            })
            .collect()
    } else {
        files
            .iter()
            .enumerate()
            .filter_map(|(ix, path)| Some((score(path, &query)?, ix)))
            .collect()
    };
    let by_score = |a: &(i64, usize), b: &(i64, usize)| {
        b.0.cmp(&a.0).then_with(|| files[a.1].cmp(&files[b.1]))
    };
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, by_score);
        scored.truncate(limit);
    }
    scored.sort_unstable_by(by_score);
    scored.into_iter().map(|(_, ix)| ix).collect()
}

fn cost(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX / 2_000)
}

/// Fuzzy score of `path` for a lowercased, whitespace-free `query`; `None` if the query is not
/// a subsequence of the path. Matches in the file name beat matches in directories;
/// contiguous runs and word starts score higher; shorter paths win ties.
fn score(path: &str, query: &[char]) -> Option<i64> {
    let name_start = path.rfind('/').map_or(0, |ix| ix + 1);
    let name = &path[name_start..];
    let name_score = subsequence_score(name, query).map(|s| s + 1_000);
    let path_score = || subsequence_score(path, query);
    let best = name_score.or_else(path_score)?;
    let substring_bonus = if name
        .to_lowercase()
        .contains(&query.iter().collect::<String>())
    {
        500
    } else {
        0
    };
    Some(best + substring_bonus - cost(path.len()))
}

/// Greedy left-to-right subsequence match with bonuses for consecutive characters and word
/// starts (after `/ _ - .` or a lower→upper case change).
fn subsequence_score(text: &str, query: &[char]) -> Option<i64> {
    let mut score = 0;
    let mut q = 0;
    let mut previous: Option<char> = None;
    let mut last_match: Option<usize> = None;
    for (ix, ch) in text.chars().enumerate() {
        if q == query.len() {
            break;
        }
        if ch.to_lowercase().eq(std::iter::once(query[q])) {
            score += 10;
            if last_match.is_some_and(|last| last + 1 == ix) {
                score += 15;
            }
            let boundary = match previous {
                None => true,
                Some(p) => {
                    matches!(p, '/' | '_' | '-' | '.' | ' ')
                        || (p.is_lowercase() && ch.is_uppercase())
                }
            };
            if boundary {
                score += 20;
            }
            last_match = Some(ix);
            q += 1;
        }
        previous = Some(ch);
    }
    (q == query.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::rank;

    fn files() -> Vec<String> {
        [
            "README.md",
            "crates/app/src/main.rs",
            "crates/app/src/chat/composer.rs",
            "crates/app/src/chat/mod.rs",
            "crates/term/src/terminal.rs",
            "docs/compose-guide.md",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn top(query: &str) -> Vec<String> {
        let files = files();
        rank(&files, query, 3)
            .into_iter()
            .map(|ix| files[ix].clone())
            .collect()
    }

    #[test]
    fn prefers_file_name_and_contiguous_matches() {
        assert_eq!(top("composer")[0], "crates/app/src/chat/composer.rs");
        assert_eq!(top("main")[0], "crates/app/src/main.rs");
        assert_eq!(top("term")[0], "crates/term/src/terminal.rs");
        // A directory-only match still counts, below file-name matches.
        assert!(top("chatmod").contains(&"crates/app/src/chat/mod.rs".to_owned()));
        assert_eq!(top("zzz"), Vec::<String>::new());
    }

    #[test]
    fn case_and_spaces_are_ignored_and_results_capped() {
        assert_eq!(top("Read Me")[0], "README.md");
        assert_eq!(top("").len(), 3);
        assert_eq!(top("")[0], "README.md");
        assert_eq!(top("s").len(), 3);
    }
}
