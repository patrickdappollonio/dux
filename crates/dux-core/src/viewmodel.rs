//! Serializable projection of `Engine` state for web clients. Selection, focus,
//! and scroll position are intentionally excluded — those are client-side state
//! under the independent-navigation model. This is a one-way `core -> client`
//! view; it never deserializes.

use serde::Serialize;

use crate::engine::Engine;
use crate::model::{AgentSession, PrInfo, PrState, Project, ProjectBranchStatus, ProviderKind};
use crate::worker::{ResourceKind, ResourceStats};

/// The projects/sessions/sidebar "spine" a web client reads via `GET /api/v1/spine`
/// (and the thin per-resource reads `GET /api/v1/projects`, `GET /api/v1/sessions`,
/// `GET /api/v1/sessions/:id`). Refetched when a coarse `projects.changed` or
/// `sessions.changed` event fires. Changed files are served separately via
/// `GET /api/v1/sessions/:id/changes` (signaled by `session.changes`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpineView {
    pub projects: Vec<ProjectView>,
    pub sessions: Vec<SessionView>,
    /// Core-computed sidebar grouping (projects + sessions, with orphaned
    /// sessions surfaced) so both surfaces render an identical tree without
    /// re-deriving grouping at the interface.
    pub sidebar: crate::sidebar::SidebarModel,
}

/// The build-/config-static snapshot a web client fetches ONCE on load via
/// `GET /api/v1/bootstrap`. These fields change only on a config reload, which
/// signals the client to refetch with a `config.changed` event (see
/// [`Engine::bootstrap`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BootstrapView {
    /// Configured provider command names, sorted. Surfaced so a client can
    /// populate a per-project default-provider picker.
    pub available_providers: Vec<String>,
    /// Text macros from `[macros]` in `config.toml`, in config (IndexMap) order.
    /// The web surfaces these two ways: the terminal-pane quick-picker filters
    /// by the focused target's surface and runs one via `RunMacro`, and the
    /// macro-editor dialog lists/edits them (which is why `text` is exposed —
    /// the web session is authenticated). A config reload that changes `[macros]`
    /// rebuilds this, delivered by a `config.changed` refetch.
    pub macros: Vec<MacroView>,
    /// Web-surface welcome-screen tips, from the shared `dux_core::welcome` list.
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
    /// Mirrors the raw `config.ui.github_integration` flag — distinct from
    /// `gh_available`, which is the composite (integration on AND `gh`
    /// installed/authed). Distinct from `gh_available` so the web can reason
    /// about the user's raw preference independently of whether `gh` is
    /// currently reachable (a banner the user can't currently see is still a
    /// real preference once `gh` comes back).
    pub github_integration: bool,
    /// Mirrors `config.ui.copy_on_select`: whether selecting text in the web
    /// terminal auto-copies it (default true). Read by the terminal pane and by
    /// the web's Preferences dialog.
    pub copy_on_select: bool,
    /// Mirrors `config.ui.attention_grace_seconds`: seconds the attention
    /// indicators stay visible in the web UI after the browser tab returns to
    /// the foreground, before the focused agent's needs-attention flag
    /// clears (default 3; 0 clears immediately). Web-only; read by the
    /// terminal pane's viewed-ping scheduling.
    pub attention_grace_seconds: u64,
    /// Mirrors `config.capabilities.web_notifications`: whether the web UI bridges
    /// an agent's notification sequences to a browser desktop Notification (default
    /// true). The browser still only fires after the visitor grants permission and
    /// only while the tab is backgrounded. Older servers omit it (treated as true).
    pub web_notifications: bool,
    /// Mirrors `config.capabilities.hyperlinks`: whether the web terminal renders
    /// OSC 8 hyperlinks as clickable (http/https only). Older servers omit it
    /// (treated as true).
    pub hyperlinks: bool,
    /// The normalized `config.capabilities.clipboard_passthrough` mode ("focused",
    /// "always", or "off"), governing whether an agent's OSC 52 clipboard SET
    /// reaches the visitor's browser clipboard. Serialized as its canonical string
    /// (an unrecognized config value normalizes to "focused"). Older servers omit
    /// it, so the web client falls back to "focused".
    pub clipboard_passthrough: String,
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
    /// Mirrors `config.ui.show_changes_pane`. The desktop web hides the
    /// right-hand Changes pane when false; the Changes actions menu's runtime
    /// toggle overrides it per session. Older servers omit it, so the web treats a
    /// missing value as `true`.
    pub show_changes_pane: bool,
    /// Global environment variables from `[env]` in `config.toml`, applied to
    /// every spawned provider/terminal. Surfaced so a client can pre-fill an
    /// edit dialog.
    pub global_env: std::collections::BTreeMap<String, String>,
    /// Mirrors `config.ui.status_clear_seconds`. The web honors it for toast
    /// auto-dismiss: an info/success toast clears this many seconds after it
    /// arrives (0 disables auto-clear, matching the TUI's tone-aware policy).
    /// Warning/error toasts ignore it and persist until replaced.
    pub status_clear_seconds: u16,
    /// Mirrors `config.server.title`: the operator-chosen display name for this
    /// dux instance. The web shows it as the browser tab title and the brand
    /// wordmark above the version in the projects pane, and resolves an
    /// empty/whitespace value to "dux". Older servers omit it (the web treats a
    /// missing value as "dux").
    pub title: String,
    /// Mirrors `config.server.favicon`: empty means the original full-color yellow
    /// duck; one of the curated tint colors (violet, blue, sky, cyan, teal, green,
    /// amber, orange, red, pink, rose) recolors a flat duck silhouette. The web
    /// resolves and applies it; an unrecognized value falls back to the default
    /// duck. Older servers omit it (the web treats a missing value as the default).
    pub favicon: String,
    /// The per-agent tab cap (`config.ui.agent_tabs_max`, normalized/clamped),
    /// INCLUDING the session-slot tab. The web disables the "+" add-tab affordance once a
    /// session already has this many tabs; the server re-enforces it on create.
    /// Older servers omit it, so the web falls back to a sane default.
    pub agent_tabs_max: u16,
    /// Mirrors `config.ui.always_show_tab_strip`: when true the web always
    /// renders the agent tab strip, even when a session has only one tab.
    /// Default false shows it only once a session has two or more tabs.
    /// Changing it from the web's Preferences dialog persists the new value
    /// here.
    /// Older servers omit it, so the web treats a missing value as `false`.
    pub always_show_tab_strip: bool,
    /// Mirrors `config.ui.attention_indicator`: whether an attention
    /// glyph/dot/tab-title/favicon cue is shown at all when an agent asks for
    /// attention (default true). The settings modal's "Both surfaces" group
    /// exposes this alongside `attention_on_bell`. Older servers omit it, so
    /// the web treats a missing value as `true`. Keep this in sync with the web
    /// `settingsDescriptors.ts` (see the cross-reference comment there).
    pub attention_indicator: bool,
    /// Mirrors `config.ui.attention_on_bell`: whether a plain terminal bell is
    /// also treated as an attention request (default true; has no effect when
    /// `attention_indicator` is false). Older servers omit it, so the web
    /// treats a missing value as `true`.
    pub attention_on_bell: bool,
    /// Mirrors `config.defaults.provider`: the GLOBAL default provider for new
    /// agents in projects without a project-specific override, matching the
    /// TUI's `change-default-provider` palette command. Distinct from
    /// `ProjectView::default_provider`, which is the effective PER-PROJECT
    /// value (global default or project override). Named `global_` here so it
    /// cannot be confused with that per-project field when read alongside it.
    /// Older servers omit it, so the web falls back to "claude".
    pub global_default_provider: String,
}

/// A single text macro projected for web clients, from
/// `dux_core::config::MacroEntry`. Order in [`BootstrapView::macros`] matches the
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
    /// Project terminals open at this project's repo root (owned by the project,
    /// with no agent attached), sorted by `id` for stability. Session-owned
    /// companion terminals live on `SessionView::terminals` instead.
    pub terminals: Vec<TerminalView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub project_id: String,
    pub title: Option<String>,
    pub provider: String,
    pub branch_name: String,
    /// The branch this agent was created on, immutable. Distinct from
    /// `branch_name` (the current branch, which tracks the worktree). When they
    /// differ the current branch has drifted since creation.
    pub initial_branch: String,
    /// The branch this agent was forked from (its fork point / leading branch).
    pub source_branch: String,
    pub worktree_path: String,
    /// "active" | "detached" | "exited"
    pub status: String,
    pub auto_reopen_enabled: bool,
    /// Associated GitHub pull request, if one is tracked for this session.
    pub pr: Option<PrView>,
    /// Companion terminals open for this session, sorted by `id` for stability.
    pub terminals: Vec<TerminalView>,
    /// Provider tabs for this session, **Main first** (`tabs[0]`, `id ==
    /// session id`) then extra tabs in creation order. Always non-empty. The
    /// client shows the tab strip only when `tabs.len() >= 2`; with one tab the
    /// pane looks exactly as it did before tabs existed.
    pub tabs: Vec<AgentTabView>,
    /// Whether the session's PTY has emitted any output yet. The web UI shows a
    /// readiness spinner until this is true.
    pub has_output: bool,
    /// Whether the agent is actively streaming output right now (PTY data within
    /// [`crate::engine::AGENT_STREAMING_WINDOW`]). This is a *hysteresis boolean*,
    /// not a timestamp: it stays `true` for the whole window after the latest
    /// byte and flips back to `false` only once the window lapses. Coarse
    /// `sessions.changed` events coalesce, so a steadily streaming agent produces
    /// a stable `working: true` until a transition (idle→working or
    /// working→idle) occurs.
    pub working: bool,
    /// Whether any of this agent's tabs currently needs attention (a permission
    /// prompt, a finished turn) that the user has not yet looked at. Rolled up
    /// any-tab, mirroring `working`. Memory-only runtime state; the web surfaces
    /// this as a sidebar dot, a browser-tab count, and a favicon dot.
    pub needs_attention: bool,
    /// Session creation time as an RFC 3339 / ISO 8601 string. Exposed so the
    /// web client can compute the same sort orders the TUI offers
    /// (`sort-agents-by-created`) and feed the result back through
    /// `reorder_sessions`. Both surfaces persist into the shared order, so a
    /// sort on either stays in sync by construction.
    pub created_at: String,
    /// Session last-update time as an RFC 3339 / ISO 8601 string. Mirror of
    /// `created_at`; backs the web's `sort-agents-by-updated` parity command.
    pub updated_at: String,
    /// The tab id the user last focused on this agent, verbatim from
    /// [`crate::model::AgentSession::last_focused_tab`]. `None` (or a value
    /// naming a tab no longer in `tabs`) means "no memory" and callers should
    /// resolve to the session-slot tab (`id`) — see
    /// `crates/dux-web/web/src/lib/agentTabs.ts`'s `resolveFocusedTab` for the
    /// web-side resolver and [`crate::model::AgentSession::resolved_focused_tab`]
    /// for the shared rule.
    pub last_focused_tab: Option<String>,
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
    /// slowly and the coarse `sessions.changed` signal stays calm. The web UI
    /// shows this as the terminal's title when present, falling back to `label`.
    pub foreground_cmd: Option<String>,
}

/// One provider tab of an agent, projected for the tab strip. `order == 0` is
/// the **session-slot tab** (the only resumable one). extra tabs (`order >= 1`) are
/// ephemeral and always launch fresh.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentTabView {
    /// Tab id. Equals the session id for the session-slot tab.
    pub id: String,
    /// Effective provider name (the running pin if a swap happened while live,
    /// otherwise the tab's configured provider).
    pub provider: String,
    /// Position in the strip: 0 = session-slot tab, 1..N = extra tabs in creation
    /// order. Display ordering only — no tab is privileged (resume is decided
    /// dynamically at launch by liveness, not by position).
    pub order: u32,
    /// Whether this tab's PTY is actively streaming (per-tab hysteresis boolean).
    pub working: bool,
    /// Whether this specific tab needs attention (unacknowledged). The tab strip
    /// marks the flagged tab's pill; the sidebar rolls this up across tabs.
    pub needs_attention: bool,
    /// Whether this tab's PTY has emitted any output yet.
    pub has_output: bool,
    /// Whether a live PTY exists for this tab right now. `false` for a dormant
    /// extra tab (e.g. reopened after a restart) — the web client renders the
    /// dormant card from this flag *without* subscribing, because subscribing
    /// would force-launch the provider.
    pub has_live_process: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrView {
    pub number: u64,
    /// "open" | "merged" | "closed"
    pub state: String,
    pub title: String,
    pub url: String,
}

/// A single changed file projected for web clients (used by the per-session
/// changed-files REST read `GET /api/v1/sessions/:id/changes`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangedFileView {
    pub status: String,
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

/// One process inside a sampled tree, projected for web clients.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessInfoView {
    pub name: String,
    pub pid: u32,
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    /// True for the entry that IS the row's root process. The breakdown
    /// includes the root so it sums to the row total; this marks it so it does
    /// not read as a phantom duplicate of the row above.
    pub is_root: bool,
}

/// One resource-monitor row projected for web clients (`GET /api/v1/resources`).
///
/// Deliberately a separate type from [`crate::worker::ResourceStats`]: that is
/// the engine's sampling type, this is the wire contract, and the browser joins
/// a row to the spine by `id` rather than by parsing `label`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceStatsView {
    /// The spine id to join on: a tab id for `agent`, a terminal id for
    /// `terminal`. Absent for the `dux` and `total` rows, which describe no
    /// single spine entity.
    pub id: Option<String>,
    /// One of `dux`, `agent`, `terminal`, `total`.
    pub kind: String,
    /// Human-readable description. Display only; never parse it.
    pub label: String,
    pub pid: Option<u32>,
    /// May exceed 100: a multi-threaded tree spread across cores legitimately
    /// does, so no surface may clamp it.
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    pub process_count: usize,
    pub children: Vec<ProcessInfoView>,
    /// Whether the breakdown carries information beyond the row itself, i.e.
    /// whether the client should offer an expand affordance. Computed by core
    /// (`ResourceStats::has_breakdown`) rather than re-derived from
    /// `children.length` in the browser, so the two surfaces cannot drift on
    /// the rule: `children` always contains the root, so the threshold is
    /// `> 1`, and a leaf's lone entry is not a breakdown.
    pub has_breakdown: bool,
}

impl ResourceStatsView {
    /// Project sampled engine rows onto the wire, preserving order (the
    /// collector emits dux first and total last).
    pub fn from_stats(rows: Vec<ResourceStats>) -> Vec<Self> {
        rows.into_iter()
            .map(|r| Self {
                has_breakdown: r.has_breakdown(),
                id: r.id,
                kind: match r.kind {
                    ResourceKind::Dux => "dux",
                    ResourceKind::Agent => "agent",
                    ResourceKind::Terminal => "terminal",
                    ResourceKind::Total => "total",
                }
                .to_string(),
                label: r.label,
                pid: r.pid,
                cpu_percent: r.cpu_percent,
                rss_bytes: r.rss_bytes,
                process_count: r.process_count,
                children: r
                    .children
                    .into_iter()
                    .map(|c| ProcessInfoView {
                        name: c.name,
                        pid: c.pid,
                        cpu_percent: c.cpu_percent,
                        rss_bytes: c.rss_bytes,
                        is_root: c.is_root,
                    })
                    .collect(),
            })
            .collect()
    }
}

impl ProjectView {
    fn from_project(p: &Project, terminals: Vec<TerminalView>) -> Self {
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
            terminals,
        }
    }
}

impl SessionView {
    #[allow(clippy::too_many_arguments)]
    fn from_session(
        s: &AgentSession,
        pr: Option<&PrInfo>,
        terminals: Vec<TerminalView>,
        tabs: Vec<AgentTabView>,
        has_output: bool,
        working: bool,
        needs_attention: bool,
    ) -> Self {
        Self {
            id: s.id.clone(),
            project_id: s.project_id.clone(),
            title: s.title.clone(),
            provider: s.provider.as_str().to_string(),
            branch_name: s.branch_name.clone(),
            initial_branch: s.initial_branch.clone(),
            source_branch: s.source_branch.clone(),
            worktree_path: s.worktree_path.clone(),
            status: s.status.as_str().to_string(),
            auto_reopen_enabled: s.auto_reopen_enabled,
            pr: pr.map(PrView::from_pr),
            terminals,
            tabs,
            has_output,
            working,
            needs_attention,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            last_focused_tab: s.last_focused_tab.clone(),
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
    /// Project the projects/sessions/sidebar spine served via `GET /api/v1/spine`
    /// (and the thin per-resource reads). This is the exact projection logic the
    /// [`Engine::view_model`] used to inline for those three fields; it was factored
    /// out so the REST read and the change-detection that emits
    /// `projects.changed`/`sessions.changed` share one source of truth.
    pub fn spine(&self) -> SpineView {
        // Group extra tabs by session id in ONE pass (O(total tabs)) so each
        // per-session projection costs only its own tab count, instead of every
        // `project_session` re-scanning the whole `agent_tabs` map (O(S * T)).
        let mut support_by_session: std::collections::HashMap<&str, Vec<&crate::model::AgentTab>> =
            std::collections::HashMap::new();
        for t in self.agent_tabs.values() {
            support_by_session
                .entry(t.session_id.as_str())
                .or_default()
                .push(t);
        }
        SpineView {
            projects: self
                .projects
                .iter()
                .map(|p| ProjectView::from_project(p, self.project_terminal_views(&p.id)))
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(|s| {
                    let support = support_by_session
                        .get(s.id.as_str())
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]);
                    self.project_session(s, support)
                })
                .collect(),
            sidebar: crate::sidebar::build_sidebar(
                &self.projects,
                &self.sessions,
                &self.project_ids_with_terminals(),
                self.config.ui.empty_project_separator_min_projects,
            ),
        }
    }

    /// The project-owned terminals of `project_id` as [`TerminalView`]s, sorted
    /// by id for stability (mirrors the session-terminal projection below).
    fn project_terminal_views(&self, project_id: &str) -> Vec<TerminalView> {
        let mut terminals: Vec<TerminalView> = self
            .companion_terminals
            .iter()
            .filter(
                |(_, t)| matches!(&t.owner, crate::model::TerminalOwner::Project(pid) if pid == project_id),
            )
            .map(|(id, t)| TerminalView {
                id: id.clone(),
                label: t.label.clone(),
                has_output: t.client.has_output(),
                foreground_cmd: t.foreground_cmd.clone(),
            })
            .collect();
        terminals.sort_by(|a, b| a.id.cmp(&b.id));
        terminals
    }

    /// The set of project ids that own at least one live project terminal. Feeds
    /// the sidebar split so a project with a live terminal never sinks below the
    /// "no agents" separator.
    pub fn project_ids_with_terminals(&self) -> std::collections::HashSet<String> {
        self.companion_terminals
            .values()
            .filter_map(|t| match &t.owner {
                crate::model::TerminalOwner::Project(pid) => Some(pid.clone()),
                crate::model::TerminalOwner::Session(_) => None,
            })
            .collect()
    }

    /// Project a single session into its [`SessionView`], looking up its companion
    /// terminals, PR status, output, and streaming flag exactly as [`Engine::spine`]
    /// does. Factored out so the per-session REST read (`GET /api/v1/sessions/:id`)
    /// can project ONLY the requested session instead of building the whole spine.
    fn project_session(
        &self,
        s: &AgentSession,
        support_tabs: &[&crate::model::AgentTab],
    ) -> SessionView {
        let mut terminals: Vec<TerminalView> = self
            .companion_terminals
            .iter()
            .filter(
                |(_, t)| matches!(&t.owner, crate::model::TerminalOwner::Session(sid) if *sid == s.id),
            )
            .map(|(id, t)| TerminalView {
                id: id.clone(),
                label: t.label.clone(),
                has_output: t.client.has_output(),
                foreground_cmd: t.foreground_cmd.clone(),
            })
            .collect();
        terminals.sort_by(|a, b| a.id.cmp(&b.id));
        // The sidebar-facing status reflects ANY live tab: the agent is "active"
        // when any of its tabs (session-slot or extra) has a live PTY. (The
        // persisted `desired_running` auto-reopen intent stays agent-level and is
        // NOT churned by transient per-tab activity — that's set/cleared on the
        // delete/detach paths, not here.)
        // Derive tab ids from the already-computed `support_tabs` slice (plus the
        // session-slot id) instead of re-scanning the whole `agent_tabs` map via
        // `tab_ids_for_session` — that full scan is exactly what `support_tabs`
        // was precomputed to avoid (`spine`'s single O(total tabs) grouping pass),
        // and calling it here per-session silently re-introduced the O(S * T) cost
        // `spine` was factored to eliminate.
        let has_output = std::iter::once(s.id.as_str())
            .chain(support_tabs.iter().map(|t| t.id.as_str()))
            .any(|id| self.providers.get(id).is_some_and(|p| p.has_output()));
        let working = std::iter::once(s.id.as_str())
            .chain(support_tabs.iter().map(|t| t.id.as_str()))
            .any(|id| self.is_agent_streaming(id));
        // Attention rolls up any-tab, exactly like `working`: the sidebar row
        // marks the agent if any of its tabs (session-slot or extra) is flagged.
        let needs_attention = std::iter::once(s.id.as_str())
            .chain(support_tabs.iter().map(|t| t.id.as_str()))
            .any(|id| self.tab_needs_attention(id));
        // Tabs, session-slot first, then extras in creation order.
        let mut tabs = vec![self.tab_view(&s.id, self.running_provider_for(s), 0)];
        let mut support: Vec<_> = support_tabs.to_vec();
        support.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        for (i, t) in support.into_iter().enumerate() {
            let effective = self
                .running_provider_pins
                .get(&t.id)
                .cloned()
                .unwrap_or_else(|| t.provider.clone());
            tabs.push(self.tab_view(&t.id, effective, (i + 1) as u32));
        }
        SessionView::from_session(
            s,
            self.pr_statuses.get(&s.id),
            terminals,
            tabs,
            has_output,
            working,
            needs_attention,
        )
    }

    /// Project one tab id into an [`AgentTabView`] from the tab-keyed runtime maps.
    /// `order` is display position only; no tab is privileged.
    fn tab_view(&self, id: &str, provider: ProviderKind, order: u32) -> AgentTabView {
        AgentTabView {
            id: id.to_string(),
            provider: provider.as_str().to_string(),
            order,
            working: self.is_agent_streaming(id),
            needs_attention: self.tab_needs_attention(id),
            has_output: self
                .providers
                .get(id)
                .map(|p| p.has_output())
                .unwrap_or(false),
            has_live_process: self.providers.contains_key(id),
        }
    }

    /// Project ONLY the session with `id` into a [`SessionView`], or `None` if no
    /// such session exists. Serves `GET /api/v1/sessions/:id` without building the
    /// whole projects/sessions/sidebar spine just to find one session.
    pub fn session_view(&self, id: &str) -> Option<SessionView> {
        self.sessions.iter().find(|s| s.id == id).map(|s| {
            let support: Vec<&crate::model::AgentTab> = self
                .agent_tabs
                .values()
                .filter(|t| t.session_id == s.id)
                .collect();
            self.project_session(s, &support)
        })
    }

    /// Project the build-/config-static snapshot served once via
    /// `GET /api/v1/bootstrap`. Delivered as a one-shot REST read invalidated by
    /// `config.changed`.
    pub fn bootstrap(&self) -> BootstrapView {
        let mut available_providers: Vec<String> =
            self.config.providers.commands.keys().cloned().collect();
        available_providers.sort();
        BootstrapView {
            available_providers,
            macros: self
                .config
                .macros
                .entries
                .iter()
                .map(|(name, entry)| MacroView::from_entry(name, entry))
                .collect(),
            welcome_tips: crate::welcome::web_tips(),
            dux_version: crate::display_version().to_string(),
            randomize_agent_names_by_default: self
                .config
                .defaults
                .enable_randomized_pet_name_by_default,
            gh_available: self.pr_agent_command_available(),
            github_integration: self.config.ui.github_integration,
            copy_on_select: self.config.ui.copy_on_select,
            attention_grace_seconds: self.config.ui.attention_grace_seconds,
            web_notifications: self.config.capabilities.web_notifications,
            hyperlinks: self.config.capabilities.hyperlinks,
            clipboard_passthrough: crate::config::ClipboardPassthroughMode::parse(
                &self.config.capabilities.clipboard_passthrough,
            )
            .unwrap_or(crate::config::ClipboardPassthroughMode::Focused)
            .as_str()
            .to_string(),
            pr_banner_position: self.config.ui.pr_banner_position.clone(),
            agent_scrollback_lines: self.config.ui.agent_scrollback_lines,
            show_changes_pane: self.config.ui.show_changes_pane,
            global_env: self.config.env.clone(),
            status_clear_seconds: self.config.ui.status_clear_seconds,
            title: self.config.server.title.clone(),
            favicon: self.config.server.favicon.clone(),
            agent_tabs_max: self.agent_tabs_max(),
            always_show_tab_strip: self.config.ui.always_show_tab_strip,
            attention_indicator: self.config.ui.attention_indicator,
            attention_on_bell: self.config.ui.attention_on_bell,
            global_default_provider: self.config.defaults.provider.clone(),
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
        assert!(!engine.bootstrap().dux_version.is_empty());
    }

    #[test]
    fn projects_and_sessions_are_projected_on_the_spine() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        // Projects and sessions live on the spine projection.
        let spine = engine.spine();
        assert_eq!(spine.projects.len(), 1);
        assert_eq!(spine.projects[0].id, "p1");
        assert_eq!(spine.projects[0].default_provider, "claude");
        assert_eq!(spine.projects[0].branch_status, "leading");
        assert_eq!(spine.sessions.len(), 1);
        assert_eq!(spine.sessions[0].id, "s1");
        assert_eq!(spine.sessions[0].branch_name, "feature");
        assert_eq!(spine.sessions[0].status, "detached");
    }

    #[test]
    fn session_view_exposes_initial_and_source_branch() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut s = sample_session("s1", "p1", "cur");
        s.initial_branch = "orig".into();
        s.source_branch = "main".into();
        engine.sessions.push(s);

        let view = &engine.spine().sessions[0];
        assert_eq!(view.branch_name, "cur");
        assert_eq!(view.initial_branch, "orig");
        assert_eq!(view.source_branch, "main");
    }

    #[test]
    fn welcome_tips_are_projected_on_bootstrap() {
        let (engine, _tmp) = test_engine();
        assert!(
            !engine.bootstrap().welcome_tips.is_empty(),
            "welcome_tips should carry the shared web tips"
        );
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

        let boot = engine.bootstrap();
        let names: Vec<&str> = boot.macros.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["zebra", "alpha", "beta"]);
        assert_eq!(boot.macros[0].text, "z text");
        // Surface serializes with the lowercase config serde casing.
        assert_eq!(boot.macros[0].surface, "agent");
        assert_eq!(boot.macros[1].surface, "terminal");
        assert_eq!(boot.macros[2].surface, "both");
    }

    #[test]
    fn macros_reflect_a_config_reload() {
        use crate::config::{MacroEntry, MacroSurface};
        let (mut engine, _tmp) = test_engine();
        assert!(engine.bootstrap().macros.is_empty());

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

        let boot = engine.bootstrap();
        assert_eq!(boot.macros.len(), 1);
        assert_eq!(boot.macros[0].name, "fresh");
        assert_eq!(boot.macros[0].text, "reloaded");
        assert_eq!(boot.macros[0].surface, "both");
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

        let vm = engine.spine();

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

        let vm = engine.spine();

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

        let vm = engine.spine();

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

        let vm = engine.spine();

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

        let vm = engine.spine();

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

        let vm = engine.spine();

        assert_eq!(vm.sessions[0].created_at, created.to_rfc3339());
        assert_eq!(vm.sessions[0].updated_at, updated.to_rfc3339());
    }

    #[test]
    fn session_without_provider_is_not_working() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.spine();

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

        let vm = engine.spine();

        assert!(
            vm.sessions[0].working,
            "a session stamped with fresh PTY activity should project working=true"
        );
    }

    #[test]
    fn any_tab_activity_lights_the_sidebar() {
        use std::time::Instant;

        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        // An extra tab of s1, with its own id.
        engine.agent_tabs.insert(
            "t1".to_string(),
            crate::model::AgentTab {
                id: "t1".to_string(),
                session_id: "s1".to_string(),
                provider: crate::model::ProviderKind::new("codex"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        // Only the extra tab is streaming; the session-slot tab (s1) is idle.
        engine.pty_activity.insert("t1".to_string(), Instant::now());

        let vm = engine.spine();
        let session = &vm.sessions[0];

        // Sidebar status now reflects ANY tab: an extra tab streaming lights the
        // row even though the session-slot tab is idle (no tab is privileged).
        assert!(
            session.working,
            "activity on any tab must light the sidebar"
        );

        // Tabs: session-slot first, then the extra tab, each with its own flags.
        assert_eq!(session.tabs.len(), 2);
        assert_eq!(session.tabs[0].id, "s1");
        assert_eq!(session.tabs[0].order, 0);
        assert!(!session.tabs[0].working);
        assert_eq!(session.tabs[1].id, "t1");
        assert_eq!(session.tabs[1].order, 1);
        assert_eq!(session.tabs[1].provider, "codex");
        assert!(session.tabs[1].working, "the extra tab itself is streaming");
        assert!(
            !session.tabs[1].has_live_process,
            "no PtyClient was inserted"
        );
    }

    #[test]
    fn single_tab_session_projects_just_the_session_slot_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.spine();
        assert_eq!(vm.sessions[0].tabs.len(), 1);
        assert_eq!(vm.sessions[0].tabs[0].id, "s1");
        assert_eq!(vm.sessions[0].tabs[0].order, 0);
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

        let vm = engine.spine();
        let terminals = &vm.sessions[0].terminals;
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].id, terminal_id);
        assert_eq!(terminals[0].label, label);
        // A freshly-created terminal has no foreground command yet.
        assert_eq!(terminals[0].foreground_cmd, None);
    }

    #[test]
    fn spine_projects_project_terminals_onto_project_view_not_sessions() {
        let (mut engine, _tmp) = test_engine();

        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = repo.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_project_terminal("p1")
            .expect("create project terminal");

        let vm = engine.spine();
        // The project carries the terminal...
        let project_terminals = &vm.projects[0].terminals;
        assert_eq!(project_terminals.len(), 1);
        assert_eq!(project_terminals[0].id, terminal_id);
        assert_eq!(project_terminals[0].label, label);
        // ...and the session does NOT (the owner filter must not mix up).
        assert!(
            vm.sessions[0].terminals.is_empty(),
            "a project terminal must never be projected onto a session"
        );
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

        let vm = engine.spine();
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

        let vm = engine.spine();
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

        let vm = engine.spine();

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
        assert!(!engine.spine().sessions[0].has_output);

        engine
            .providers
            .get("s1")
            .expect("provider exists")
            .write_bytes(b"hello\n")
            .expect("write to provider");

        // Poll for up to ~2s while the reader thread processes the echo.
        let mut became_ready = false;
        for _ in 0..40 {
            if engine.spine().sessions[0].has_output {
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

        let boot = engine.bootstrap();

        assert_eq!(boot.global_env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn randomize_agent_names_default_is_projected() {
        let (mut engine, _tmp) = test_engine();

        // Defaults to false out of the box.
        assert!(!engine.bootstrap().randomize_agent_names_by_default);

        engine.config.defaults.enable_randomized_pet_name_by_default = true;
        assert!(engine.bootstrap().randomize_agent_names_by_default);
    }

    #[test]
    fn agent_scrollback_lines_is_projected() {
        let (mut engine, _tmp) = test_engine();

        engine.config.ui.agent_scrollback_lines = 4242;
        assert_eq!(engine.bootstrap().agent_scrollback_lines, 4242);
    }

    #[test]
    fn status_clear_seconds_is_projected() {
        let (mut engine, _tmp) = test_engine();

        // Default ships at 6 seconds.
        assert_eq!(engine.bootstrap().status_clear_seconds, 6);

        engine.config.ui.status_clear_seconds = 0;
        assert_eq!(engine.bootstrap().status_clear_seconds, 0);

        engine.config.ui.status_clear_seconds = 42;
        assert_eq!(engine.bootstrap().status_clear_seconds, 42);
    }

    #[test]
    fn server_title_is_projected() {
        let (mut engine, _tmp) = test_engine();
        // Defaults flow through unchanged.
        assert_eq!(engine.bootstrap().title, "dux");
        // A configured instance name reaches the bootstrap view verbatim (the web
        // resolves empty/whitespace to "dux"; the projection itself is faithful).
        engine.config.server.title = "dux #1".to_string();
        assert_eq!(engine.bootstrap().title, "dux #1");
    }

    #[test]
    fn server_favicon_is_projected() {
        let (mut engine, _tmp) = test_engine();
        // Default is empty (the web treats that as "bundled logo").
        assert_eq!(engine.bootstrap().favicon, "");
        // A configured value reaches the bootstrap view verbatim; the web
        // interprets it (colour vs URL vs default).
        engine.config.server.favicon = "violet".to_string();
        assert_eq!(engine.bootstrap().favicon, "violet");
    }

    #[test]
    fn gh_available_reflects_integration_and_gh_status() {
        let (mut engine, _tmp) = test_engine();

        // Out of the box: integration off, gh status unknown -> unavailable.
        assert!(!engine.bootstrap().gh_available);

        // Integration on but gh not yet confirmed available -> still false.
        engine.github_integration_enabled = true;
        assert!(!engine.bootstrap().gh_available);

        // Integration on AND gh available -> true.
        engine.gh_status = crate::model::GhStatus::Available;
        assert!(engine.bootstrap().gh_available);

        // gh present but integration disabled -> false (the TUI gating).
        engine.github_integration_enabled = false;
        assert!(!engine.bootstrap().gh_available);
    }

    #[test]
    fn available_providers_lists_configured_defaults_sorted() {
        let (engine, _tmp) = test_engine();

        let boot = engine.bootstrap();

        // A default Config configures these four providers.
        for provider in ["claude", "codex", "copilot", "opencode"] {
            assert!(
                boot.available_providers.iter().any(|p| p == provider),
                "available_providers should contain {provider}: {:?}",
                boot.available_providers
            );
        }
        // The list is sorted.
        let mut sorted = boot.available_providers.clone();
        sorted.sort();
        assert_eq!(boot.available_providers, sorted);
    }

    #[test]
    fn show_changes_pane_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // Default ships visible.
        assert!(engine.bootstrap().show_changes_pane);

        engine.config.ui.show_changes_pane = false;
        assert!(!engine.bootstrap().show_changes_pane);
    }

    #[test]
    fn always_show_tab_strip_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // Default ships off (strip shows only with 2+ tabs).
        assert!(!engine.bootstrap().always_show_tab_strip);

        engine.config.ui.always_show_tab_strip = true;
        assert!(engine.bootstrap().always_show_tab_strip);
    }

    #[test]
    fn pr_banner_position_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // The default config ships with the banner at the bottom.
        assert_eq!(engine.bootstrap().pr_banner_position, "bottom");

        // An explicit "top" preference projects verbatim so the web client can
        // mirror the TUI's placement.
        engine.config.ui.pr_banner_position = "top".to_string();
        assert_eq!(engine.bootstrap().pr_banner_position, "top");
    }

    #[test]
    fn attention_indicator_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // Default ships on.
        assert!(engine.bootstrap().attention_indicator);

        engine.config.ui.attention_indicator = false;
        assert!(!engine.bootstrap().attention_indicator);
    }

    #[test]
    fn attention_on_bell_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        assert!(engine.bootstrap().attention_on_bell);

        engine.config.ui.attention_on_bell = false;
        assert!(!engine.bootstrap().attention_on_bell);
    }

    #[test]
    fn global_default_provider_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        assert_eq!(engine.bootstrap().global_default_provider, "claude");

        engine.config.defaults.provider = "codex".to_string();
        assert_eq!(engine.bootstrap().global_default_provider, "codex");
    }

    #[test]
    fn bootstrap_serializes_to_json_with_expected_fields() {
        let (engine, _tmp) = test_engine();
        let json = serde_json::to_string(&engine.bootstrap()).expect("serialize");
        for field in [
            "available_providers",
            "macros",
            "welcome_tips",
            "dux_version",
            "randomize_agent_names_by_default",
            "gh_available",
            "github_integration",
            "copy_on_select",
            "attention_grace_seconds",
            "web_notifications",
            "hyperlinks",
            "clipboard_passthrough",
            "pr_banner_position",
            "agent_scrollback_lines",
            "show_changes_pane",
            "global_env",
            "title",
            "favicon",
            "status_clear_seconds",
            "always_show_tab_strip",
            "attention_indicator",
            "attention_on_bell",
            "global_default_provider",
        ] {
            assert!(
                json.contains(&format!("\"{field}\"")),
                "bootstrap JSON must carry {field}: {json}"
            );
        }
    }

    #[test]
    fn spine_serializes_to_json() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let vm = engine.spine();
        let json = serde_json::to_string(&vm).expect("serialize");
        assert!(json.contains("\"id\":\"p1\""), "json: {json}");
        assert!(
            json.contains("\"branch_status\":\"leading\""),
            "json: {json}"
        );
    }
}
