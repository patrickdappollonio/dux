//! Serializable projection of `Engine` state for web clients. Selection, focus,
//! and scroll position are intentionally excluded — those are client-side state
//! under the independent-navigation model. This is a one-way `core -> client`
//! view; it never deserializes.

use serde::Serialize;

use crate::engine::Engine;
use crate::model::{AgentSession, ChangedFile, PrInfo, PrState, Project, ProjectBranchStatus};

/// The full chrome snapshot a web client needs to draw projects, sessions, and
/// the changed-files lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewModel {
    pub projects: Vec<ProjectView>,
    pub sessions: Vec<SessionView>,
    /// Core-computed sidebar grouping (projects + sessions, with orphaned
    /// sessions surfaced) so both surfaces render an identical tree without
    /// re-deriving grouping at the interface.
    pub sidebar: crate::sidebar::SidebarModel,
    pub changed_files: ChangedFilesView,
    /// Global environment variables from `[env]` in `config.toml`, applied to
    /// every spawned provider/terminal. Surfaced so a client can pre-fill an
    /// edit dialog.
    pub global_env: std::collections::BTreeMap<String, String>,
    /// Configured provider command names, sorted. Surfaced so a client can
    /// populate a per-project default-provider picker.
    pub available_providers: Vec<String>,
    /// Web-surface welcome-screen tips, from the shared `dux_core::welcome`
    /// list. Static content; the watch channel coalesces identical frames so
    /// this does not cause churn.
    pub welcome_tips: Vec<String>,
    /// Mirrors the binary's display version ('vX.Y.Z' or 'development'); the web shows it in the sidebar brand block.
    pub dux_version: String,
    /// Mirrors `defaults.enable_randomized_pet_name_by_default`. When true, the
    /// web new-agent dialog pre-checks its "Use randomized pet name" box (and
    /// requests a generated name on open), matching the TUI's prompt default.
    pub randomize_agent_names_by_default: bool,
    /// Whether the new-agent-from-PR flow is available (GitHub integration on +
    /// `gh` installed and authenticated; see `Engine::pr_agent_command_available`).
    /// The web dialog hides/disables its "From PR" mode with a quiet explanation
    /// when false, matching the TUI's gating of the `new-agent-from-pr` command.
    pub gh_available: bool,
    /// Mirrors `config.ui.pr_banner_position` ("top" | "bottom"). Desktop web
    /// places the PR banner lane above the terminal when "top" and below it when
    /// "bottom", matching the TUI's `pr_banner_at_bottom` semantics. Mobile
    /// ignores this and always renders the banner on top.
    pub pr_banner_position: String,
    /// Mirrors `config.ui.agent_scrollback_lines`. The web sizes each xterm.js
    /// instance's scrollback to this so it can retain the full history the
    /// reconnect repaint replays — without it, xterm.js silently caps at its
    /// 1000-line default and trims the replayed history.
    pub agent_scrollback_lines: usize,
    /// Surface-aware command-palette commands that the web should render as a
    /// global "Commands" group, in canonical registry order. Derived from
    /// `dux_core::palette` (the `Web`/`Both` subset). Each entry's `id` is the
    /// dashed command name; the web's `paletteRegistry` maps it to a store
    /// handler. Static for a given build — the watch channel coalesces identical
    /// frames, so this does not cause churn.
    pub palette_commands: Vec<PaletteCommandView>,
    /// Text macros from `[macros]` in `config.toml`, in config (IndexMap) order.
    /// The web surfaces these two ways: the terminal-pane quick-picker filters
    /// by the focused target's surface and runs one via `RunMacro`, and the
    /// macro-editor dialog lists/edits them (which is why `text` is exposed —
    /// the web session is authenticated). A config reload that changes `[macros]`
    /// rebuilds this, so the coalesced ViewModel watch pushes the new list.
    pub macros: Vec<MacroView>,
}

/// A single text macro projected for web clients, from
/// `dux_core::config::MacroEntry`. Order in [`ViewModel::macros`] matches the
/// config `IndexMap`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MacroView {
    /// The macro's name (its `[macros]` key).
    pub name: String,
    /// The macro's expansion text (may be multi-line).
    pub text: String,
    /// "agent" | "terminal" | "both" — matches the config serde casing for
    /// `MacroSurface`.
    pub surface: String,
}

/// A single global palette command surfaced to the web, projected from
/// `dux_core::palette::PaletteCommand`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaletteCommandView {
    /// The dashed command name (e.g. `sort-agents-by-updated`). Stable id used
    /// to look up the web handler in `paletteRegistry`.
    pub id: &'static str,
    /// One-line description shown alongside the id in the palette.
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default_provider: String,
    /// Explicit per-project provider override (None = inherits the global default).
    pub explicit_default_provider: Option<String>,
    pub auto_reopen_agents: Option<bool>,
    pub startup_command: Option<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub current_branch: String,
    /// "leading" | "not_leading" | "unknown"
    pub branch_status: String,
    pub path_missing: bool,
    /// The project's configured leading/default branch, if known. Surfaced so a
    /// client can show the default branch in a project-info view. `None` when it
    /// has not been detected yet (e.g. a missing checkout).
    pub leading_branch: Option<String>,
    /// When this project was first added, as an RFC 3339 / ISO 8601 string.
    /// Empty when no store row exists yet (a freshly constructed project that
    /// has not been persisted). Surfaced so a client can show an "added" date.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub project_id: String,
    pub title: Option<String>,
    pub provider: String,
    pub branch_name: String,
    pub worktree_path: String,
    /// "active" | "detached" | "exited"
    pub status: String,
    pub auto_reopen_enabled: bool,
    /// Associated GitHub pull request, if one is tracked for this session.
    pub pr: Option<PrView>,
    /// Companion terminals open for this session, sorted by `id` for stability.
    pub terminals: Vec<TerminalView>,
    /// Whether the session's PTY has emitted any output yet. The web UI shows a
    /// readiness spinner until this is true.
    pub has_output: bool,
    /// Whether the agent is actively streaming output right now (PTY data within
    /// [`crate::engine::AGENT_STREAMING_WINDOW`]). This is a *hysteresis boolean*,
    /// not a timestamp: it stays `true` for the whole window after the latest
    /// byte and flips back to `false` only once the window lapses. Because the
    /// ViewModel watch channel coalesces identical frames (`send_if_modified`),
    /// a steadily streaming agent produces a stable `working: true` and pushes
    /// nothing until a transition (idle→working or working→idle) occurs.
    pub working: bool,
    /// Session creation time as an RFC 3339 / ISO 8601 string. Exposed so the
    /// web client can compute the same sort orders the TUI offers
    /// (`sort-agents-by-created`) and feed the result back through
    /// `reorder_sessions`. Both surfaces persist into the shared order, so a
    /// sort on either stays in sync by construction.
    pub created_at: String,
    /// Session last-update time as an RFC 3339 / ISO 8601 string. Mirror of
    /// `created_at`; backs the web's `sort-agents-by-updated` parity command.
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalView {
    pub id: String,
    pub label: String,
    /// Whether the terminal's PTY has emitted any output yet.
    pub has_output: bool,
    /// The command currently running in the foreground of this terminal, or
    /// `None` when the shell itself is idle in the foreground. Projected verbatim
    /// from [`crate::model::CompanionTerminal::foreground_cmd`], which the engine
    /// refreshes at most every ~2s
    /// ([`crate::engine::FOREGROUND_REFRESH_INTERVAL`]) — so this field changes
    /// slowly and the coalesced ViewModel watch stays calm. The web UI shows
    /// this as the terminal's title when present, falling back to `label`.
    pub foreground_cmd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrView {
    pub number: u64,
    /// "open" | "merged" | "closed"
    pub state: String,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ChangedFilesView {
    pub staged: Vec<ChangedFileView>,
    pub unstaged: Vec<ChangedFileView>,
    /// The session id these lists belong to (the currently watched worktree), or
    /// `None` when nothing is watched. A web client renders these lists only when
    /// this matches its locally selected session — otherwise it shows a loading
    /// state rather than another session's files (cross-tab safety).
    pub watched_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangedFileView {
    pub status: String,
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

impl ProjectView {
    fn from_project(p: &Project) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.clone(),
            default_provider: p.default_provider.as_str().to_string(),
            explicit_default_provider: p
                .explicit_default_provider
                .as_ref()
                .map(|pk| pk.as_str().to_string()),
            auto_reopen_agents: p.auto_reopen_agents,
            startup_command: p.startup_command.clone(),
            env: p.env.clone(),
            current_branch: p.current_branch.clone(),
            branch_status: match p.branch_status {
                ProjectBranchStatus::Leading => "leading",
                ProjectBranchStatus::NotLeading => "not_leading",
                ProjectBranchStatus::Unknown => "unknown",
            }
            .to_string(),
            path_missing: p.path_missing,
            leading_branch: p.leading_branch.clone(),
            created_at: p.created_at.map(|dt| dt.to_rfc3339()).unwrap_or_default(),
        }
    }
}

impl SessionView {
    fn from_session(
        s: &AgentSession,
        pr: Option<&PrInfo>,
        terminals: Vec<TerminalView>,
        has_output: bool,
        working: bool,
    ) -> Self {
        Self {
            id: s.id.clone(),
            project_id: s.project_id.clone(),
            title: s.title.clone(),
            provider: s.provider.as_str().to_string(),
            branch_name: s.branch_name.clone(),
            worktree_path: s.worktree_path.clone(),
            status: s.status.as_str().to_string(),
            auto_reopen_enabled: s.auto_reopen_enabled,
            pr: pr.map(PrView::from_pr),
            terminals,
            has_output,
            working,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
        }
    }
}

impl PrView {
    fn from_pr(pr: &PrInfo) -> Self {
        Self {
            number: pr.number,
            state: match pr.state {
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
            }
            .to_string(),
            title: pr.title.clone(),
            url: pr.url.clone(),
        }
    }
}

impl ChangedFileView {
    fn from_file(f: &ChangedFile) -> Self {
        Self {
            status: f.status.clone(),
            path: f.path.clone(),
            additions: f.additions,
            deletions: f.deletions,
            binary: f.binary,
        }
    }
}

impl MacroView {
    fn from_entry(name: &str, entry: &crate::config::MacroEntry) -> Self {
        Self {
            name: name.to_string(),
            text: entry.text.clone(),
            surface: entry.surface.as_config_str().to_string(),
        }
    }
}

impl Engine {
    /// Project current engine state into a serializable snapshot for web clients.
    pub fn view_model(&self) -> ViewModel {
        let mut available_providers: Vec<String> =
            self.config.providers.commands.keys().cloned().collect();
        available_providers.sort();
        ViewModel {
            projects: self
                .projects
                .iter()
                .map(ProjectView::from_project)
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(|s| {
                    let mut terminals: Vec<TerminalView> = self
                        .companion_terminals
                        .iter()
                        .filter(|(_, t)| t.session_id == s.id)
                        .map(|(id, t)| TerminalView {
                            id: id.clone(),
                            label: t.label.clone(),
                            has_output: t.client.has_output(),
                            foreground_cmd: t.foreground_cmd.clone(),
                        })
                        .collect();
                    terminals.sort_by(|a, b| a.id.cmp(&b.id));
                    let has_output = self
                        .providers
                        .get(&s.id)
                        .map(|p| p.has_output())
                        .unwrap_or(false);
                    let working = self.is_agent_streaming(&s.id);
                    SessionView::from_session(
                        s,
                        self.pr_statuses.get(&s.id),
                        terminals,
                        has_output,
                        working,
                    )
                })
                .collect(),
            sidebar: crate::sidebar::build_sidebar(
                &self.projects,
                &self.sessions,
                self.config.ui.empty_project_separator_min_projects,
            ),
            changed_files: ChangedFilesView {
                staged: self
                    .staged_files
                    .iter()
                    .map(ChangedFileView::from_file)
                    .collect(),
                unstaged: self
                    .unstaged_files
                    .iter()
                    .map(ChangedFileView::from_file)
                    .collect(),
                watched_session_id: self.watched_session_id.clone(),
            },
            global_env: self.config.env.clone(),
            available_providers,
            welcome_tips: crate::welcome::web_tips(),
            dux_version: crate::display_version().to_string(),
            randomize_agent_names_by_default: self
                .config
                .defaults
                .enable_randomized_pet_name_by_default,
            gh_available: self.pr_agent_command_available(),
            pr_banner_position: self.config.ui.pr_banner_position.clone(),
            agent_scrollback_lines: self.config.ui.agent_scrollback_lines,
            palette_commands: crate::palette::web_palette_commands()
                .map(|c| PaletteCommandView {
                    id: c.name,
                    description: c.description,
                })
                .collect(),
            macros: self
                .config
                .macros
                .entries
                .iter()
                .map(|(name, entry)| MacroView::from_entry(name, entry))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    #[test]
    fn dux_version_is_projected() {
        let (engine, _tmp) = test_engine();
        assert!(!engine.view_model().dux_version.is_empty());
    }

    #[test]
    fn projects_sessions_and_changed_files_are_projected() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        engine.staged_files.push(ChangedFile {
            status: "M".to_string(),
            path: "src/lib.rs".to_string(),
            additions: 3,
            deletions: 1,
            binary: false,
        });

        let vm = engine.view_model();

        assert_eq!(vm.projects.len(), 1);
        assert_eq!(vm.projects[0].id, "p1");
        assert_eq!(vm.projects[0].default_provider, "claude");
        assert_eq!(vm.projects[0].branch_status, "leading");
        assert_eq!(vm.sessions.len(), 1);
        assert_eq!(vm.sessions[0].id, "s1");
        assert_eq!(vm.sessions[0].branch_name, "feature");
        assert_eq!(vm.sessions[0].status, "detached");
        assert_eq!(vm.changed_files.staged.len(), 1);
        assert_eq!(vm.changed_files.staged[0].path, "src/lib.rs");
        assert_eq!(vm.changed_files.staged[0].additions, 3);
        assert_eq!(vm.changed_files.unstaged.len(), 0);
        assert!(
            !vm.welcome_tips.is_empty(),
            "welcome_tips should carry the shared web tips"
        );
    }

    #[test]
    fn palette_commands_project_web_subset_in_registry_order() {
        let (engine, _tmp) = test_engine();
        let vm = engine.view_model();

        // The projected ids equal the Web/Both subset of the core registry, in
        // canonical registry order.
        let expected: Vec<&str> = crate::palette::web_palette_commands()
            .map(|c| c.name)
            .collect();
        let actual: Vec<&str> = vm.palette_commands.iter().map(|c| c.id).collect();
        assert_eq!(actual, expected);

        // Descriptions are carried verbatim from the registry.
        for (view, cmd) in vm
            .palette_commands
            .iter()
            .zip(crate::palette::web_palette_commands())
        {
            assert_eq!(view.id, cmd.name);
            assert_eq!(view.description, cmd.description);
        }

        // Spot-check that a known web command is present and a known TUI-only
        // command is absent.
        assert!(actual.contains(&"add-project"));
        assert!(actual.contains(&"toggle-diff-line-numbers"));
        assert!(!actual.contains(&"start-web-server"));
        assert!(!actual.contains(&"change-theme"));
    }

    #[test]
    fn macros_are_projected_in_config_order_with_serde_surface_casing() {
        use crate::config::{MacroEntry, MacroSurface};
        let (mut engine, _tmp) = test_engine();
        // Insert in a non-alphabetical order to prove IndexMap order is preserved.
        engine.config.macros.entries.insert(
            "zebra".to_string(),
            MacroEntry {
                text: "z text".to_string(),
                surface: MacroSurface::Agent,
            },
        );
        engine.config.macros.entries.insert(
            "alpha".to_string(),
            MacroEntry {
                text: "a text".to_string(),
                surface: MacroSurface::Terminal,
            },
        );
        engine.config.macros.entries.insert(
            "beta".to_string(),
            MacroEntry {
                text: "b text".to_string(),
                surface: MacroSurface::Both,
            },
        );

        let vm = engine.view_model();
        let names: Vec<&str> = vm.macros.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["zebra", "alpha", "beta"]);
        assert_eq!(vm.macros[0].text, "z text");
        // Surface serializes with the lowercase config serde casing.
        assert_eq!(vm.macros[0].surface, "agent");
        assert_eq!(vm.macros[1].surface, "terminal");
        assert_eq!(vm.macros[2].surface, "both");
    }

    #[test]
    fn macros_reflect_a_config_reload() {
        use crate::config::{MacroEntry, MacroSurface};
        let (mut engine, _tmp) = test_engine();
        assert!(engine.view_model().macros.is_empty());

        // Simulate a config reload that introduces a new macro.
        let mut new_config = engine.config.clone();
        new_config.macros.entries.insert(
            "fresh".to_string(),
            MacroEntry {
                text: "reloaded".to_string(),
                surface: MacroSurface::Both,
            },
        );
        engine
            .apply_reloaded_config(new_config)
            .expect("apply reloaded config");

        let vm = engine.view_model();
        assert_eq!(vm.macros.len(), 1);
        assert_eq!(vm.macros[0].name, "fresh");
        assert_eq!(vm.macros[0].text, "reloaded");
        assert_eq!(vm.macros[0].surface, "both");
    }

    #[test]
    fn project_settings_fields_are_projected() {
        use crate::model::ProviderKind;

        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/repo");
        project.explicit_default_provider = Some(ProviderKind::new("codex"));
        project.auto_reopen_agents = Some(true);
        project.startup_command = Some("npm install".to_string());
        project.env.insert("KEY".to_string(), "value".to_string());
        engine.projects.push(project);

        let vm = engine.view_model();

        let p = &vm.projects[0];
        assert_eq!(p.explicit_default_provider.as_deref(), Some("codex"));
        assert_eq!(p.auto_reopen_agents, Some(true));
        assert_eq!(p.startup_command.as_deref(), Some("npm install"));
        assert_eq!(p.env.get("KEY").map(String::as_str), Some("value"));
    }

    #[test]
    fn project_without_settings_has_none() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        let vm = engine.view_model();

        let p = &vm.projects[0];
        assert!(p.explicit_default_provider.is_none());
        assert!(p.auto_reopen_agents.is_none());
        assert!(p.startup_command.is_none());
        assert!(p.env.is_empty());
    }

    #[test]
    fn project_leading_branch_and_created_at_are_projected() {
        let (mut engine, _tmp) = test_engine();

        // A project with a known leading branch and a stored created_at.
        let mut with = sample_project("p1", "/repo");
        with.leading_branch = Some("trunk".to_string());
        let added = chrono::DateTime::parse_from_rfc3339("2026-02-03T04:05:06+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        with.created_at = Some(added);
        engine.projects.push(with);

        // A project with no detected leading branch and no store row yet.
        let mut without = sample_project("p2", "/repo2");
        without.leading_branch = None;
        without.created_at = None;
        engine.projects.push(without);

        let vm = engine.view_model();

        assert_eq!(vm.projects[0].leading_branch.as_deref(), Some("trunk"));
        assert_eq!(vm.projects[0].created_at, added.to_rfc3339());
        assert!(vm.projects[1].leading_branch.is_none());
        assert_eq!(
            vm.projects[1].created_at, "",
            "a project with no store row projects an empty created_at"
        );
    }

    #[test]
    fn session_pr_status_is_projected() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        engine.pr_statuses.insert(
            "s1".to_string(),
            PrInfo {
                number: 42,
                state: PrState::Merged,
                title: "Add the thing".to_string(),
                host: "github.com".to_string(),
                owner_repo: "owner/repo".to_string(),
                url: "https://github.com/owner/repo/pull/42".to_string(),
            },
        );

        let vm = engine.view_model();

        let pr = vm.sessions[0]
            .pr
            .as_ref()
            .expect("session should carry projected PR");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, "merged");
        assert_eq!(pr.title, "Add the thing");
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn session_without_pr_has_none() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.view_model();

        assert!(vm.sessions[0].pr.is_none());
    }

    #[test]
    fn session_timestamps_are_projected_as_rfc3339() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feature");
        let created = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let updated = chrono::DateTime::parse_from_rfc3339("2026-03-04T05:06:07+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        session.created_at = created;
        session.updated_at = updated;
        engine.sessions.push(session);

        let vm = engine.view_model();

        assert_eq!(vm.sessions[0].created_at, created.to_rfc3339());
        assert_eq!(vm.sessions[0].updated_at, updated.to_rfc3339());
    }

    #[test]
    fn session_without_provider_is_not_working() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.view_model();

        assert!(
            !vm.sessions[0].working,
            "a session with no PTY activity should project working=false"
        );
    }

    #[test]
    fn session_with_recent_activity_is_working() {
        use std::time::Instant;

        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        // Stamp the activity map directly rather than spinning up a real
        // PtyClient: the projection only reads `is_agent_streaming`, which keys
        // off this map, so a fresh timestamp is sufficient and avoids spawning
        // a child process in a unit test.
        engine.pty_activity.insert("s1".to_string(), Instant::now());

        let vm = engine.view_model();

        assert!(
            vm.sessions[0].working,
            "a session stamped with fresh PTY activity should project working=true"
        );
    }

    #[test]
    fn companion_terminals_are_projected_onto_their_session() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");

        let vm = engine.view_model();
        let terminals = &vm.sessions[0].terminals;
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].id, terminal_id);
        assert_eq!(terminals[0].label, label);
        // A freshly-created terminal has no foreground command yet.
        assert_eq!(terminals[0].foreground_cmd, None);
    }

    #[test]
    fn terminal_foreground_cmd_is_projected_verbatim() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, _label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");

        // Set the model field directly (the engine's wall-clock-throttled probe
        // is exercised separately; here we just prove the projection copies it).
        engine
            .companion_terminals
            .get_mut(&terminal_id)
            .expect("terminal exists")
            .foreground_cmd = Some("npm".to_string());

        let vm = engine.view_model();
        assert_eq!(
            vm.sessions[0].terminals[0].foreground_cmd.as_deref(),
            Some("npm"),
            "a Some foreground_cmd must project verbatim"
        );

        // Clearing the model field projects back to null.
        engine
            .companion_terminals
            .get_mut(&terminal_id)
            .expect("terminal exists")
            .foreground_cmd = None;

        let vm = engine.view_model();
        assert_eq!(
            vm.sessions[0].terminals[0].foreground_cmd, None,
            "a None foreground_cmd must project as null"
        );
    }

    #[test]
    fn session_without_provider_is_not_ready() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.view_model();

        assert!(!vm.sessions[0].has_output);
    }

    #[test]
    fn running_provider_marks_session_ready() {
        use std::time::Duration;

        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // Spawn a real `cat` PTY as the session's provider. `cat` echoes input,
        // so writing to it guarantees the child emits output we can latch on.
        let client = crate::pty::PtyClient::spawn_with_env(
            "cat",
            &[],
            worktree.path(),
            24,
            80,
            engine.config.ui.agent_scrollback_lines,
            &[],
        )
        .expect("spawn cat provider");
        engine.providers.insert("s1".to_string(), client);

        // Before any output, the session is not ready.
        assert!(!engine.view_model().sessions[0].has_output);

        engine
            .providers
            .get("s1")
            .expect("provider exists")
            .write_bytes(b"hello\n")
            .expect("write to provider");

        // Poll for up to ~2s while the reader thread processes the echo.
        let mut became_ready = false;
        for _ in 0..40 {
            if engine.view_model().sessions[0].has_output {
                became_ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert!(became_ready, "session should become ready after output");
    }

    #[test]
    fn global_env_is_projected() {
        let (mut engine, _tmp) = test_engine();
        engine
            .config
            .env
            .insert("FOO".to_string(), "bar".to_string());

        let vm = engine.view_model();

        assert_eq!(vm.global_env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn randomize_agent_names_default_is_projected() {
        let (mut engine, _tmp) = test_engine();

        // Defaults to false out of the box.
        assert!(!engine.view_model().randomize_agent_names_by_default);

        engine.config.defaults.enable_randomized_pet_name_by_default = true;
        assert!(engine.view_model().randomize_agent_names_by_default);
    }

    #[test]
    fn agent_scrollback_lines_is_projected() {
        let (mut engine, _tmp) = test_engine();

        engine.config.ui.agent_scrollback_lines = 4242;
        assert_eq!(engine.view_model().agent_scrollback_lines, 4242);
    }

    #[test]
    fn gh_available_reflects_integration_and_gh_status() {
        let (mut engine, _tmp) = test_engine();

        // Out of the box: integration off, gh status unknown -> unavailable.
        assert!(!engine.view_model().gh_available);

        // Integration on but gh not yet confirmed available -> still false.
        engine.github_integration_enabled = true;
        assert!(!engine.view_model().gh_available);

        // Integration on AND gh available -> true.
        engine.gh_status = crate::model::GhStatus::Available;
        assert!(engine.view_model().gh_available);

        // gh present but integration disabled -> false (the TUI gating).
        engine.github_integration_enabled = false;
        assert!(!engine.view_model().gh_available);
    }

    #[test]
    fn available_providers_lists_configured_defaults_sorted() {
        let (engine, _tmp) = test_engine();

        let vm = engine.view_model();

        // A default Config configures these four providers.
        for provider in ["claude", "codex", "gemini", "opencode"] {
            assert!(
                vm.available_providers.iter().any(|p| p == provider),
                "available_providers should contain {provider}: {:?}",
                vm.available_providers
            );
        }
        // The list is sorted.
        let mut sorted = vm.available_providers.clone();
        sorted.sort();
        assert_eq!(vm.available_providers, sorted);
    }

    #[test]
    fn pr_banner_position_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // The default config ships with the banner at the bottom.
        assert_eq!(engine.view_model().pr_banner_position, "bottom");

        // An explicit "top" preference projects verbatim so the web client can
        // mirror the TUI's placement.
        engine.config.ui.pr_banner_position = "top".to_string();
        assert_eq!(engine.view_model().pr_banner_position, "top");
    }

    #[test]
    fn view_model_serializes_to_json() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let vm = engine.view_model();
        let json = serde_json::to_string(&vm).expect("serialize");
        assert!(json.contains("\"id\":\"p1\""), "json: {json}");
        assert!(
            json.contains("\"branch_status\":\"leading\""),
            "json: {json}"
        );
    }
}
