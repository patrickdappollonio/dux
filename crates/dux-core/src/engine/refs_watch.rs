//! The refs watcher: notices an agent's branch moving (a commit, a push
//! updating its remote-tracking ref, a `git pack-refs`) and asks for a
//! pull-request check for that agent there and then.
//!
//! What it watches is resolved on a worker, because finding where a branch's
//! refs live means asking git: in a linked worktree `.git` is a FILE, and the
//! refs every worktree of a repository shares live in the repository's common
//! git directory. The set of agents it covers is the pull-request plan's own,
//! re-resolved whenever that plan's agents or branches change, so an agent is
//! watched from the moment it is created and stops being watched when it goes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::engine::Engine;
use crate::engine::events::EventReaction;
use crate::engine::spawn_worker::BackgroundWorkerSpec;
use crate::worker::WorkerEvent;

/// How long after one notification for an agent further ref moves for it are
/// dropped. A commit and its push move two refs a moment apart, and one check
/// answers for both.
const REFS_CHANGE_DEBOUNCE: Duration = Duration::from_secs(5);

/// An agent the watcher should cover, as the pull-request plan names it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefsWatchTarget {
    pub session_id: String,
    pub worktree_path: String,
    pub branch: String,
}

/// Where one agent's refs live on disk, resolved by git on a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRefsWatch {
    pub session_id: String,
    pub files: crate::git::BranchRefFiles,
}

/// The live watcher and what it currently covers.
pub struct RefsWatcher {
    watcher: notify::RecommendedWatcher,
    /// Every ref file whose change means something, mapped to the agents it is
    /// about. Shared with the watcher's callback, which routes on exact paths.
    routes: Arc<Mutex<HashMap<PathBuf, Vec<String>>>>,
    /// The directories under a watch, and how.
    watched_dirs: HashMap<PathBuf, RecursiveMode>,
    /// The agents last asked to be resolved, so an unchanged plan asks nothing.
    requested: Option<Vec<RefsWatchTarget>>,
    /// Bumped on every request, so an answer overtaken by a newer one is
    /// dropped rather than applied.
    generation: u64,
}

impl Engine {
    /// Create the refs watcher, if it does not exist yet, and resolve what it
    /// should watch for the agents the pull-request plan covers now.
    pub fn spawn_refs_watcher(&mut self) {
        if self.refs_watcher.is_none() {
            let routes: Arc<Mutex<HashMap<PathBuf, Vec<String>>>> = Arc::default();
            let callback_routes = Arc::clone(&routes);
            let tx = self.worker_tx.clone();
            let mut last_sent: HashMap<String, Instant> = HashMap::new();
            let watcher = notify::RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| {
                    let Ok(event) = res else { return };
                    // A ref is written as `<ref>.lock` and renamed over the ref,
                    // so the move shows up as a create or a rename naming the
                    // ref's own path.
                    if !event.kind.is_modify() && !event.kind.is_create() {
                        return;
                    }
                    let Ok(routes) = callback_routes.lock() else {
                        return;
                    };
                    for event_path in &event.paths {
                        let Some(session_ids) = routes.get(event_path) else {
                            continue;
                        };
                        for session_id in session_ids {
                            let now = Instant::now();
                            if last_sent.get(session_id).is_some_and(|last| {
                                now.duration_since(*last) < REFS_CHANGE_DEBOUNCE
                            }) {
                                continue;
                            }
                            last_sent.insert(session_id.clone(), now);
                            crate::logger::debug(&format!(
                                "[gh-integration] refs watcher: {} moved, checking session {}",
                                event_path.display(),
                                session_id,
                            ));
                            let _ = tx.send(WorkerEvent::RefsChanged(session_id.clone()));
                        }
                    }
                },
                notify::Config::default(),
            );
            match watcher {
                Ok(watcher) => {
                    self.refs_watcher = Some(RefsWatcher {
                        watcher,
                        routes,
                        watched_dirs: HashMap::new(),
                        requested: None,
                        generation: 0,
                    });
                }
                Err(e) => {
                    crate::logger::warn(&format!(
                        "[gh-integration] refs watcher: failed to create watcher (falling back to poll-only): {e}",
                    ));
                    // The fallback is silent otherwise, and a user watching pull
                    // request status arrive a poll interval late has no way to
                    // tell that from dux being broken.
                    self.post_status(crate::poller_status::refs_watcher_unavailable(
                        &e.to_string(),
                    ));
                    return;
                }
            }
        }
        self.request_refs_watch_plan();
    }

    /// Ask a worker to resolve where the refs of the agents in the
    /// pull-request plan live, when that set of agents and branches differs
    /// from the one last asked about. A no-op until the watcher exists.
    pub(crate) fn request_refs_watch_plan(&mut self) {
        let Some(refs_watcher) = self.refs_watcher.as_mut() else {
            return;
        };
        let mut targets: Vec<RefsWatchTarget> = match self.pr_sync_sessions.lock() {
            Ok(plan) => plan
                .iter()
                .map(|entry| RefsWatchTarget {
                    session_id: entry.session_id.clone(),
                    worktree_path: entry.worktree_path.clone(),
                    branch: entry.branch_name.clone(),
                })
                .collect(),
            Err(_) => return,
        };
        targets.sort();
        if refs_watcher.requested.as_ref() == Some(&targets) {
            return;
        }
        refs_watcher.generation += 1;
        refs_watcher.requested = Some(targets.clone());
        let generation = refs_watcher.generation;
        self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: "refs-watch-plan".into(),
                in_flight_key: None,
                // A panic leaves the previous watch set in place; the next
                // change to the plan asks again.
                panic_event: None,
            },
            move |tx| {
                let resolved = targets
                    .iter()
                    .filter_map(|target| {
                        match crate::git::branch_ref_files(
                            Path::new(&target.worktree_path),
                            &target.branch,
                        ) {
                            Ok(files) => Some(ResolvedRefsWatch {
                                session_id: target.session_id.clone(),
                                files,
                            }),
                            Err(err) => {
                                crate::logger::debug(&format!(
                                    "[gh-integration] refs watcher: not watching session {}: {err:#}",
                                    target.session_id,
                                ));
                                None
                            }
                        }
                    })
                    .collect();
                let _ = tx.send(WorkerEvent::RefsWatchResolved {
                    generation,
                    resolved,
                });
            },
        );
    }

    /// Apply a resolved watch plan: watch what it needs, stop watching what it
    /// no longer needs, and route each ref file to its agents.
    pub(crate) fn process_refs_watch_resolved(
        &mut self,
        generation: u64,
        resolved: Vec<ResolvedRefsWatch>,
    ) -> EventReaction {
        let Some(refs_watcher) = self.refs_watcher.as_mut() else {
            return EventReaction::Nothing;
        };
        if generation != refs_watcher.generation {
            return EventReaction::Nothing;
        }
        // Each repository is watched once however many agents share it: its
        // `refs` tree recursively (a branch name with slashes nests, and a
        // first push creates the remote's directories), and the common
        // directory itself for `packed-refs`.
        let mut wanted: HashMap<PathBuf, RecursiveMode> = HashMap::new();
        for entry in &resolved {
            wanted.insert(
                entry.files.common_dir.join("refs"),
                RecursiveMode::Recursive,
            );
            wanted.insert(entry.files.common_dir.clone(), RecursiveMode::NonRecursive);
        }
        let stale: Vec<PathBuf> = refs_watcher
            .watched_dirs
            .keys()
            .filter(|dir| !wanted.contains_key(*dir))
            .cloned()
            .collect();
        for dir in stale {
            let _ = refs_watcher.watcher.unwatch(&dir);
            refs_watcher.watched_dirs.remove(&dir);
        }
        let mut unwatchable: HashSet<PathBuf> = HashSet::new();
        for (dir, mode) in wanted {
            if refs_watcher.watched_dirs.contains_key(&dir) {
                continue;
            }
            match refs_watcher.watcher.watch(&dir, mode) {
                Ok(()) => {
                    refs_watcher.watched_dirs.insert(dir, mode);
                }
                Err(e) => {
                    crate::logger::warn(&format!(
                        "[gh-integration] refs watcher: failed to watch {}: {e}",
                        dir.display(),
                    ));
                    unwatchable.insert(dir);
                }
            }
        }
        let mut routes: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut lost: Vec<String> = Vec::new();
        for entry in &resolved {
            let common = &entry.files.common_dir;
            if unwatchable.contains(&common.join("refs")) || unwatchable.contains(common) {
                lost.push(entry.session_id.clone());
                continue;
            }
            for path in [
                &entry.files.local_ref,
                &entry.files.remote_ref,
                &entry.files.packed_refs,
            ] {
                routes
                    .entry(path.clone())
                    .or_default()
                    .push(entry.session_id.clone());
            }
        }
        if let Ok(mut shared) = refs_watcher.routes.lock() {
            *shared = routes.clone();
        }
        self.refs_watch_paths = routes;
        crate::logger::info(&format!(
            "[gh-integration] refs watcher: watching {}",
            crate::text::count_of(self.refs_watched_sessions().len(), "session"),
        ));
        for session_id in lost {
            if let Some(session) = self.sessions.iter().find(|s| s.id == session_id) {
                let label = session.display_label().to_string();
                self.post_status(crate::poller_status::refs_watcher_lost_agent(&label));
            }
        }
        EventReaction::Nothing
    }

    /// The agents whose branch moves the watcher currently reports.
    pub fn refs_watched_sessions(&self) -> HashSet<String> {
        self.refs_watch_paths.values().flatten().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    fn git(cwd: &Path, args: &[&str]) {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_output(cwd: &Path, args: &[&str]) -> String {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// A linked worktree on a new branch, the home of a managed agent
    /// `s-<branch>` as dux makes them.
    fn add_agent(engine: &mut Engine, root: &Path, repo: &Path, branch: &str) -> PathBuf {
        let worktree = root.join("worktrees").join(branch);
        git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                worktree.to_str().unwrap(),
            ],
        );
        let mut session = sample_session(&format!("s-{branch}"), "p1", branch);
        session.workspace.as_managed_mut().unwrap().worktree_path =
            worktree.to_string_lossy().into_owned();
        engine.sessions.push(session);
        worktree
    }

    /// A repository with one commit, registered as project `p1`.
    fn repository(engine: &mut Engine, root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        repo
    }

    /// Process worker events until the latest watch plan has been applied.
    fn settle_watch_plan(engine: &mut Engine) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            let applied = matches!(
                &event,
                WorkerEvent::RefsWatchResolved { generation, .. }
                    if Some(*generation) == engine.refs_watcher.as_ref().map(|w| w.generation)
            );
            engine.process_worker_event(event);
            if applied {
                return;
            }
        }
        panic!("the watch plan was never applied");
    }

    /// Wait for the watcher to report a move of `session_id`'s branch.
    fn expect_refs_changed(engine: &mut Engine, session_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            if matches!(&event, WorkerEvent::RefsChanged(id) if id == session_id) {
                return;
            }
        }
        panic!("no refs change was reported for {session_id}");
    }

    #[test]
    fn a_linked_worktree_agent_is_watched_and_its_branch_moving_is_reported() {
        let (mut engine, tmp) = test_engine();
        let root = tmp.path().to_path_buf();
        let repo = repository(&mut engine, &root);
        let wt_a = add_agent(&mut engine, &root, &repo, "feat/a");
        let wt_b = add_agent(&mut engine, &root, &repo, "feat/b");
        // The remote-tracking ref exists from an earlier push.
        git(&wt_b, &["update-ref", "refs/remotes/origin/feat/b", "HEAD"]);
        engine.update_pr_sync_sessions();

        engine.spawn_refs_watcher();
        settle_watch_plan(&mut engine);
        let watched = engine.refs_watched_sessions();
        assert!(watched.contains("s-feat/a"), "watched: {watched:?}");
        assert!(watched.contains("s-feat/b"), "watched: {watched:?}");

        // A commit moves the branch's own ref, in the repository's common
        // directory rather than under the worktree.
        git(&wt_a, &["commit", "-q", "--allow-empty", "-m", "work"]);
        expect_refs_changed(&mut engine, "s-feat/a");

        // A push moves its remote-tracking ref, here to a commit made without
        // moving the local branch, so only the remote-tracking ref changes.
        let pushed = git_output(
            &wt_b,
            &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "work"],
        );
        git(
            &wt_b,
            &["update-ref", "refs/remotes/origin/feat/b", &pushed],
        );
        expect_refs_changed(&mut engine, "s-feat/b");
    }

    #[test]
    fn the_watch_follows_agents_created_and_deleted_after_it_started() {
        let (mut engine, tmp) = test_engine();
        let root = tmp.path().to_path_buf();
        let repo = repository(&mut engine, &root);
        add_agent(&mut engine, &root, &repo, "first");
        engine.update_pr_sync_sessions();
        engine.spawn_refs_watcher();
        settle_watch_plan(&mut engine);

        let later = add_agent(&mut engine, &root, &repo, "later");
        engine.update_pr_sync_sessions();
        settle_watch_plan(&mut engine);
        assert!(engine.refs_watched_sessions().contains("s-later"));
        git(&later, &["commit", "-q", "--allow-empty", "-m", "work"]);
        expect_refs_changed(&mut engine, "s-later");

        engine.sessions.retain(|s| s.id != "s-first");
        engine.update_pr_sync_sessions();
        settle_watch_plan(&mut engine);
        let watched = engine.refs_watched_sessions();
        assert!(!watched.contains("s-first"), "watched: {watched:?}");
        assert!(watched.contains("s-later"), "watched: {watched:?}");
    }
}
