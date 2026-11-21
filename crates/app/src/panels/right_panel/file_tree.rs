//! The lazily loaded file tree model behind the "Files" tab.
//!
//! Directory listings are cached per directory and flattened into the rows that are currently
//! visible, so the view can render them with a virtualized list. Loading itself happens
//! elsewhere (on the background executor); this model only tracks which directories need a
//! listing and accepts results, rejecting stale ones by token.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::SharedString;
use workspace::files::DirEntry;

/// One entry of a listed directory.
#[derive(Debug, PartialEq, Eq)]
pub struct Node {
    pub name: SharedString,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug)]
enum Listing {
    Loading,
    Loaded(Vec<Rc<Node>>),
    Failed(SharedString),
}

#[derive(Debug)]
struct DirState {
    listing: Listing,
    /// Token of the load in flight, if any. Results with another token are stale.
    pending: Option<u64>,
}

/// A visible row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: u16,
    pub kind: RowKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Node {
        node: Rc<Node>,
        expanded: bool,
        loading: bool,
    },
    /// Placeholder under an expanded directory without children.
    Empty,
    /// The listing of the parent directory failed.
    Error(SharedString),
}

/// What the view should show instead of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootState<'a> {
    Loading,
    Failed(&'a str),
    Ready,
}

#[derive(Debug)]
pub struct FileTree {
    root: PathBuf,
    dirs: HashMap<PathBuf, DirState>,
    expanded: HashSet<PathBuf>,
    rows: Vec<TreeRow>,
    next_token: u64,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            dirs: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            next_token: 0,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    pub fn root_state(&self) -> RootState<'_> {
        match self.dirs.get(&self.root).map(|dir| &dir.listing) {
            None | Some(Listing::Loading) => RootState::Loading,
            Some(Listing::Failed(error)) => RootState::Failed(error),
            Some(Listing::Loaded(_)) => RootState::Ready,
        }
    }

    /// Expands or collapses `dir`. Returns whether it is now expanded.
    pub fn toggle(&mut self, dir: &Path) -> bool {
        let expanded = if self.expanded.remove(dir) {
            false
        } else {
            self.expanded.insert(dir.to_path_buf());
            true
        };
        self.rebuild();
        expanded
    }

    /// Visible directories (the root and expanded directories whose ancestors are all
    /// expanded and listed) that have no listing yet, so the view should load them.
    pub fn missing(&self) -> Vec<PathBuf> {
        self.visible_dirs()
            .into_iter()
            .filter(|dir| !self.dirs.contains_key(dir))
            .collect()
    }

    /// Visible directories, root first.
    pub fn visible_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![self.root.clone()];
        let mut ix = 0;
        while ix < out.len() {
            if let Some(DirState {
                listing: Listing::Loaded(nodes),
                ..
            }) = self.dirs.get(&out[ix])
            {
                out.extend(
                    nodes
                        .iter()
                        .filter(|node| node.is_dir && self.expanded.contains(&node.path))
                        .map(|node| node.path.clone()),
                );
            }
            ix += 1;
        }
        out
    }

    /// Loaded directories whose listing may be stale after the given paths changed: the
    /// changed path itself (if it is a listed directory) and its parent. A change at the root
    /// (a rescan) or to an ignore file affects every listing.
    pub fn affected_dirs(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let everything = changed.iter().any(|path| {
            path == &self.root
                || path
                    .file_name()
                    .is_some_and(|name| name == ".gitignore" || name == ".ignore")
        });
        let mut out: Vec<PathBuf> = if everything {
            self.dirs.keys().cloned().collect()
        } else {
            let mut set = HashSet::new();
            for path in changed {
                for candidate in [Some(path.as_path()), path.parent()].into_iter().flatten() {
                    if self.dirs.contains_key(candidate) {
                        set.insert(candidate.to_path_buf());
                    }
                }
            }
            set.into_iter().collect()
        };
        out.sort();
        out
    }

    /// Forgets listings of directories that are not visible (collapsed or deleted), so they
    /// are reloaded fresh when shown again.
    pub fn prune_hidden(&mut self) {
        let visible: HashSet<PathBuf> = self.visible_dirs().into_iter().collect();
        self.dirs.retain(|dir, _| visible.contains(dir));
    }

    /// Marks `dir` as loading and returns the token its result must carry. An existing
    /// listing stays visible until the result arrives.
    pub fn begin_load(&mut self, dir: &Path) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        self.dirs
            .entry(dir.to_path_buf())
            .and_modify(|state| state.pending = Some(token))
            .or_insert(DirState {
                listing: Listing::Loading,
                pending: Some(token),
            });
        token
    }

    /// Installs a listing. Returns false (and changes nothing) when the result is stale.
    pub fn finish_load(
        &mut self,
        dir: &Path,
        token: u64,
        result: Result<Vec<DirEntry>, String>,
    ) -> bool {
        let Some(state) = self.dirs.get_mut(dir) else {
            return false;
        };
        if state.pending != Some(token) {
            return false;
        }
        state.pending = None;
        state.listing = match result {
            Ok(entries) => Listing::Loaded(
                entries
                    .into_iter()
                    .map(|entry| {
                        Rc::new(Node {
                            name: entry.name.into(),
                            path: entry.path,
                            is_dir: entry.is_dir,
                        })
                    })
                    .collect(),
            ),
            Err(error) => Listing::Failed(error.into()),
        };
        self.rebuild();
        true
    }

    /// Recomputes the flattened visible rows.
    pub fn rebuild(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len());
        self.push_children(&self.root, 0, &mut rows);
        self.rows = rows;
    }

    fn push_children(&self, dir: &Path, depth: u16, rows: &mut Vec<TreeRow>) {
        let Some(state) = self.dirs.get(dir) else {
            return;
        };
        match &state.listing {
            Listing::Loading => {}
            Listing::Failed(error) => {
                if dir != self.root {
                    rows.push(TreeRow {
                        depth,
                        kind: RowKind::Error(error.clone()),
                    });
                }
            }
            Listing::Loaded(nodes) if nodes.is_empty() => {
                if dir != self.root {
                    rows.push(TreeRow {
                        depth,
                        kind: RowKind::Empty,
                    });
                }
            }
            Listing::Loaded(nodes) => {
                for node in nodes {
                    let expanded = node.is_dir && self.expanded.contains(&node.path);
                    let loading = expanded
                        && self
                            .dirs
                            .get(&node.path)
                            .is_none_or(|state| matches!(state.listing, Listing::Loading));
                    rows.push(TreeRow {
                        depth,
                        kind: RowKind::Node {
                            node: node.clone(),
                            expanded,
                            loading,
                        },
                    });
                    if expanded {
                        self.push_children(&node.path, depth.saturating_add(1), rows);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use workspace::files::DirEntry;

    use super::{FileTree, RootState, RowKind};

    fn entry(path: &str, is_dir: bool) -> DirEntry {
        let path = PathBuf::from(path);
        DirEntry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            is_dir,
            is_symlink: false,
        }
    }

    /// `(depth, label)` per row, with `/` after expanded dirs.
    fn render(tree: &FileTree) -> Vec<(u16, String)> {
        tree.rows()
            .iter()
            .map(|row| {
                let label = match &row.kind {
                    RowKind::Node {
                        node,
                        expanded,
                        loading,
                    } => format!(
                        "{}{}{}",
                        node.name,
                        if *expanded { "/" } else { "" },
                        if *loading { "…" } else { "" }
                    ),
                    RowKind::Empty => "(empty)".to_owned(),
                    RowKind::Error(error) => format!("!{error}"),
                };
                (row.depth, label)
            })
            .collect()
    }

    fn loaded_tree() -> FileTree {
        let mut tree = FileTree::new("/r".into());
        assert_eq!(tree.missing(), vec![PathBuf::from("/r")]);
        assert_eq!(tree.root_state(), RootState::Loading);
        let token = tree.begin_load(Path::new("/r"));
        assert!(tree.finish_load(
            Path::new("/r"),
            token,
            Ok(vec![
                entry("/r/src", true),
                entry("/r/empty", true),
                entry("/r/README.md", false)
            ]),
        ));
        tree
    }

    #[test]
    fn expands_lazily_and_flattens() {
        let mut tree = loaded_tree();
        assert_eq!(tree.root_state(), RootState::Ready);
        assert_eq!(
            render(&tree),
            vec![
                (0, "src".into()),
                (0, "empty".into()),
                (0, "README.md".into())
            ]
        );
        assert_eq!(tree.missing(), Vec::<PathBuf>::new());

        assert!(tree.toggle(Path::new("/r/src")));
        assert_eq!(tree.missing(), vec![PathBuf::from("/r/src")]);
        let token = tree.begin_load(Path::new("/r/src"));
        assert_eq!(render(&tree)[0], (0, "src/…".into()));
        tree.finish_load(
            Path::new("/r/src"),
            token,
            Ok(vec![entry("/r/src/main.rs", false)]),
        );

        tree.toggle(Path::new("/r/empty"));
        let token = tree.begin_load(Path::new("/r/empty"));
        tree.finish_load(Path::new("/r/empty"), token, Ok(vec![]));

        assert_eq!(
            render(&tree),
            vec![
                (0, "src/".into()),
                (1, "main.rs".into()),
                (0, "empty/".into()),
                (1, "(empty)".into()),
                (0, "README.md".into()),
            ]
        );

        // Collapsing hides children but keeps the listing cached until pruned.
        assert!(!tree.toggle(Path::new("/r/src")));
        assert_eq!(render(&tree).len(), 4);
        assert_eq!(tree.missing(), Vec::<PathBuf>::new());
        tree.prune_hidden();
        tree.toggle(Path::new("/r/src"));
        assert_eq!(tree.missing(), vec![PathBuf::from("/r/src")]);
    }

    #[test]
    fn rejects_stale_results_and_keeps_old_listing_while_reloading() {
        let mut tree = loaded_tree();
        let old = tree.begin_load(Path::new("/r"));
        let new = tree.begin_load(Path::new("/r"));
        // Still showing the previous listing while reloading.
        assert_eq!(render(&tree).len(), 3);
        assert!(!tree.finish_load(Path::new("/r"), old, Ok(vec![])));
        assert_eq!(render(&tree).len(), 3);
        assert!(tree.finish_load(Path::new("/r"), new, Ok(vec![entry("/r/a", false)])));
        assert_eq!(render(&tree), vec![(0, "a".into())]);
        assert!(!tree.finish_load(Path::new("/r/unknown"), 99, Ok(vec![])));
    }

    #[test]
    fn errors() {
        let mut tree = FileTree::new("/r".into());
        let token = tree.begin_load(Path::new("/r"));
        tree.finish_load(Path::new("/r"), token, Err("denied".into()));
        assert_eq!(tree.root_state(), RootState::Failed("denied"));

        let mut tree = loaded_tree();
        tree.toggle(Path::new("/r/src"));
        let token = tree.begin_load(Path::new("/r/src"));
        tree.finish_load(Path::new("/r/src"), token, Err("gone".into()));
        assert_eq!(render(&tree)[1], (1, "!gone".into()));
    }

    #[test]
    fn affected_dirs() {
        let mut tree = loaded_tree();
        tree.toggle(Path::new("/r/src"));
        let token = tree.begin_load(Path::new("/r/src"));
        tree.finish_load(
            Path::new("/r/src"),
            token,
            Ok(vec![entry("/r/src/main.rs", false)]),
        );

        assert_eq!(
            tree.affected_dirs(&["/r/src/main.rs".into()]),
            vec![PathBuf::from("/r/src")]
        );
        assert_eq!(
            tree.affected_dirs(&["/r/src".into()]),
            vec![PathBuf::from("/r"), PathBuf::from("/r/src")]
        );
        assert_eq!(
            tree.affected_dirs(&["/r/other/deep/x".into()]),
            Vec::<PathBuf>::new()
        );
        assert_eq!(tree.affected_dirs(&["/r/.gitignore".into()]).len(), 2);
        assert_eq!(tree.affected_dirs(&["/r".into()]).len(), 2);
        assert_eq!(
            tree.visible_dirs(),
            vec![PathBuf::from("/r"), PathBuf::from("/r/src")]
        );
    }
}
