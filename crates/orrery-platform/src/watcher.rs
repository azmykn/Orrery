//! Best-effort filesystem live-watch. Calls `on_change` (debounced) whenever
//! watched repos change on disk, so the UI can rescan.
//!
//! Rather than watching every repo *fully* recursively (hundreds of thousands
//! of inotify watches across `node_modules` / Odoo trees), we watch a bounded,
//! targeted set:
//!
//! - each configured **root**, non-recursively → new top-level repos;
//! - each discovered **repo root**, non-recursively → top-level file changes;
//! - a **shallow BFS** of directories under each repo (capped, ignore-filtered)
//!   → nested edits/deletes (the common case for module trees) still notify;
//! - each repo's **`.git`** dir, non-recursively → `index`/`HEAD` cover the
//!   high-value signals (staging, commits, branch switches, ahead/behind).
//!
//! Degrades silently if watches can't be established.
//!
//! The watch set is computed from the config at spawn and again on every
//! [`WatcherHandle::rearm`], so repos/roots added while the app runs (Settings
//! save, New Project, Explore clone) get live events without a restart.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer};
use orrery_core::config;
use orrery_core::model::AppConfig;
use orrery_core::scan::{self, expand};

/// How deep under each repo root to register non-recursive directory watches.
/// Depth 0 is the repo root itself (already watched); 1 = immediate children,
/// etc. Three levels covers typical `models/`, `views/`, `static/src/…` layouts
/// without descending into huge generated trees.
const NESTED_WATCH_DEPTH: usize = 3;

/// Hard cap on nested directory watches *per repo* (in addition to the repo
/// root + `.git`). Keeps inotify usage bounded on giant checkouts (Odoo core).
const NESTED_WATCH_CAP: usize = 96;

/// What the watcher thread reacts to: a debounced fs change batch from the
/// debouncer, or a re-arm request from the app.
enum Msg {
    Fs(DebounceEventResult),
    Rearm,
}

/// Handle to the running watcher thread. Cheap to clone.
#[derive(Clone)]
pub struct WatcherHandle {
    tx: std::sync::mpsc::Sender<Msg>,
}

impl WatcherHandle {
    /// Ask the watcher thread to reload the config, recompute its targets and
    /// re-establish its watches. Call after anything that adds repos or roots
    /// at runtime. No-op if the thread is gone.
    pub fn rearm(&self) {
        let _ = self.tx.send(Msg::Rearm);
    }
}

fn watch_one(debouncer: &mut Debouncer<RecommendedWatcher>, path: &std::path::Path) -> bool {
    debouncer
        .watcher()
        .watch(path, RecursiveMode::NonRecursive)
        .is_ok()
}

fn ignored_dir(name: &str, ignore: &[String]) -> bool {
    // VCS metadata + configured scan ignores (node_modules, target, …).
    name == ".git" || name == ".hg" || name == ".svn" || ignore.iter().any(|p| p == name)
}

/// Shallow directory list under `repo` for nested watches. Skips configured
/// ignore names and caps the count so large trees stay cheap.
fn nested_watch_dirs(repo: &Path, ignore: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut q: VecDeque<(PathBuf, usize)> = VecDeque::new();
    q.push_back((repo.to_path_buf(), 0));
    while let Some((dir, depth)) = q.pop_front() {
        if depth >= NESTED_WATCH_DEPTH || out.len() >= NESTED_WATCH_CAP {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if out.len() >= NESTED_WATCH_CAP {
                break;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if ignored_dir(name, ignore) {
                continue;
            }
            let child = entry.path();
            out.push(child.clone());
            if depth + 1 < NESTED_WATCH_DEPTH {
                q.push_back((child, depth + 1));
            }
        }
    }
    out
}

/// The paths the watcher registers for a config: each configured root, each
/// discovered repo root + shallow nested dirs, and each repo's `.git` dir
/// (all non-recursive).
fn target_paths(cfg: &AppConfig) -> Vec<PathBuf> {
    // Configured roots → detect new top-level repos.
    let mut paths: Vec<PathBuf> = cfg.roots.iter().map(|root| expand(root)).collect();
    // Each repo's working root + shallow tree + .git → file changes and git ops.
    for repo in scan::repo_paths(&cfg.roots, cfg.scan_depth, &cfg.ignore) {
        let dotgit = repo.join(".git");
        paths.push(repo.clone());
        paths.extend(nested_watch_dirs(&repo, &cfg.ignore));
        if dotgit.is_dir() {
            paths.push(dotgit);
        }
    }
    paths
}

fn watch_targets(debouncer: &mut Debouncer<RecommendedWatcher>) -> usize {
    target_paths(&config::load())
        .iter()
        .filter(|path| watch_one(debouncer, path))
        .count()
}

/// Spawn the watcher thread. `on_change` fires on each debounced change batch
/// (the app rescans). The returned handle re-arms the watch set on demand; the
/// thread stays alive even with zero established watches so a later re-arm
/// (e.g. the first root added in Settings) can bring the watch up.
pub fn spawn(on_change: impl Fn() + Send + 'static) -> WatcherHandle {
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let handle = WatcherHandle { tx: tx.clone() };
    std::thread::spawn(move || {
        // The debouncer forwards fs batches onto the same channel re-arm
        // requests arrive on, so one recv loop serves both.
        let make_debouncer = || {
            let tx = tx.clone();
            new_debouncer(
                Duration::from_millis(900),
                move |res: DebounceEventResult| {
                    let _ = tx.send(Msg::Fs(res));
                },
            )
        };
        let Ok(mut debouncer) = make_debouncer() else {
            return;
        };
        watch_targets(&mut debouncer);

        // Keep `debouncer` alive for the life of the thread; fire only on real
        // change batches (`Msg::Fs` also carries notify errors, which we ignore
        // so a degraded watch can't spam rescans). On re-arm, drop the debouncer
        // — and with it every existing watch — and register a fresh set from
        // the current config: recreating is simpler than diffing paths, and
        // re-arms are rare (explicit user actions). Exits on disconnect.
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Fs(Ok(_events)) => on_change(),
                Msg::Fs(Err(_)) => {}
                Msg::Rearm => {
                    if let Ok(fresh) = make_debouncer() {
                        debouncer = fresh;
                        watch_targets(&mut debouncer);
                    }
                }
            }
        }
    });
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_paths_cover_roots_repos_and_dotgit() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // A plain directory (no .git) is not a repo, so it gets no watch.
        std::fs::create_dir_all(root.path().join("notes")).unwrap();

        let cfg = AppConfig {
            roots: vec![root.path().to_string_lossy().into_owned()],
            ..AppConfig::default()
        };
        let paths = target_paths(&cfg);
        assert!(paths.contains(&root.path().to_path_buf()));
        assert!(paths.contains(&repo));
        assert!(paths.contains(&repo.join(".git")));
        assert!(!paths.contains(&root.path().join("notes")));
    }

    #[test]
    fn target_paths_include_shallow_nested_dirs() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src").join("nested")).unwrap();
        std::fs::create_dir_all(repo.join("node_modules").join("pkg")).unwrap();

        let cfg = AppConfig {
            roots: vec![root.path().to_string_lossy().into_owned()],
            ignore: vec!["node_modules".into()],
            ..AppConfig::default()
        };
        let paths = target_paths(&cfg);
        assert!(paths.contains(&repo.join("src")));
        assert!(paths.contains(&repo.join("src").join("nested")));
        assert!(
            !paths
                .iter()
                .any(|p| p.ends_with("node_modules") || p.ends_with("pkg")),
            "ignored dirs must not be watched"
        );
    }

    #[test]
    fn target_paths_empty_config_yields_no_targets() {
        let cfg = AppConfig {
            roots: Vec::new(),
            ..AppConfig::default()
        };
        assert!(target_paths(&cfg).is_empty());
    }

    #[test]
    fn nested_watch_dirs_respects_cap() {
        let root = tempfile::tempdir().unwrap();
        // More children than the cap — BFS should stop at the limit.
        for i in 0..(NESTED_WATCH_CAP + 20) {
            std::fs::create_dir_all(root.path().join(format!("d{i}"))).unwrap();
        }
        let dirs = nested_watch_dirs(root.path(), &[]);
        assert!(dirs.len() <= NESTED_WATCH_CAP);
    }
}
