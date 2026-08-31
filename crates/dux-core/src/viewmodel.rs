//! Serializable projection of `Engine` state for web clients. Selection, focus,
//! and scroll position are intentionally excluded — those are client-side state
//! under the independent-navigation model. This is a one-way `core -> client`
//! view; it never deserializes.

use serde::Serialize;

use std::collections::BTreeMap;

use crate::engine::Engine;
use crate::ids::TabIdRef;
use crate::model::{AgentSession, PrInfo, PrState, Project, ProjectBranchStatus, ProviderKind};
use crate::worker::{ResourceKind, ResourceStats};

/// The projects/sessions/sidebar "spine" a web client reads via
/// `GET /api/v1/workspace` (and the thin per-resource reads
/// `GET /api/v1/projects`, `GET /api/v1/sessions`, `GET /api/v1/sessions/:id`).
///
/// The whole-document read is normally made ONCE, at boot: the web layer serves
/// it from a cached serialization that it also PUSHES to every connected client
/// over `/ws/events` whenever it is rebuilt, carrying a revision so a client can
/// order what it fetched against what it was pushed. The coarse
/// `projects.changed` / `sessions.changed` events still fire alongside, and are
/// what a page too old to read the push refetches on. Changed files are served
/// separately via `GET /api/v1/sessions/:id/changes` (signaled by
/// `session.changes`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpineView {
    pub projects: Vec<ProjectView>,
    pub sessions: Vec<SessionView>,
    /// Every companion terminal in manual `sort_order`. Each carries a tagged
    /// [`TerminalOwnerView`] so clients handle owner kinds exhaustively.
    pub terminals: Vec<TerminalView>,
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
    /// What CONFIG currently says a dropped file's path should look like for each
    /// configured provider, keyed by PROVIDER NAME (the `[providers.<name>]`
    /// block).
    ///
    /// This is the FALLBACK the browser uses for a pane with no live process to
    /// read from: a dormant tab, a tab whose launch has not reached the client
    /// yet, or an older server. It rides the bootstrap document with every other
    /// config-derived value rather than on a channel of its own, so a config
    /// reload refreshes it through the same `config.changed` refetch, which is
    /// exactly the event that can change it.
    ///
    /// PURELY a projection of config: a live process never writes into it. What
    /// a live process launched with is published per tab on the SPINE, in
    /// [`AgentTabView::drop_paste`], because that is what a launch and a
    /// termination refresh. A provider name absent from this map resolves to the
    /// default form (`bare`) and no length limit.
    pub provider_drop_paste: BTreeMap<String, DropPasteView>,
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
    /// Mirrors `defaults.copy_uncommitted_changes_by_default`. Seeds the web
    /// create dialog's "Copy uncommitted changes" checkbox; the per-agent
    /// choice rides the create request, so this stays read-only exposure.
    pub copy_uncommitted_changes_by_default: bool,
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
    /// Mirrors `config.ui.terminal_font_family`: a font name installed on the
    /// viewing device, placed ahead of dux's bundled terminal font stack.
    /// Empty means "use the bundled stack only." Web UI only.
    pub terminal_font_family: String,
    /// Mirrors `config.ui.terminal_font_size`, already normalized through
    /// [`crate::config::normalized_terminal_font_size`] so the browser never
    /// receives an out-of-range value. Web UI only.
    pub terminal_font_size: u16,
    /// Mirrors `config.ui.compose_bar`: WHEN the web's touch terminal shows the
    /// compose bar (the buffered phone typing surface with native autocorrect)
    /// and redirects a terminal tap into it. One of `"auto"` (the default,
    /// decided in the browser from `pointer: coarse`), `"always"` or `"never"`.
    ///
    /// Normalized through [`crate::config::ComposeBarMode`] on the way out, so
    /// the browser never receives a value it has no case for. Read by the
    /// terminal pane and by the web's Preferences dialog. Web-only.
    pub compose_bar: String,
    /// Mirrors `config.ui.mobile_top_bar`: whether the web's mobile terminal
    /// screens show the top bar (the back/branch header plus the agent tab
    /// strip), default true. A pure render gate read by the mobile shell and
    /// by the web's Preferences dialog. Web-only.
    pub mobile_top_bar: bool,
    /// Mirrors `config.ui.mobile_accessory_bar`: whether the web's mobile
    /// terminal screens show the accessory key bar (Esc/Tab/Ctrl/Alt/arrows),
    /// default true. A pure render gate read by the terminal pane and by the
    /// web's Preferences dialog. Web-only.
    pub mobile_accessory_bar: bool,
    /// Mirrors `config.ui.upload_write_gitignore`: whether the agent upload
    /// directory keeps a `.gitignore` holding a single `*`, so a file dropped
    /// or pasted onto an agent stays invisible to git. Published so the web's
    /// Preferences dialog can show the row's real current value rather than
    /// its documented default. Its companion `ui.upload_directory` is
    /// deliberately NOT exposed as a preference row: editing a path in a text
    /// field is a poor affordance, and doing it properly needs a directory
    /// picker. Web-only behavior, as the setting itself is.
    pub upload_write_gitignore: bool,
    /// Mirrors `config.ui.upload_pasted_text_chars`, already NORMALIZED: how
    /// many characters a text paste onto an AGENT pane may run to before the
    /// web saves it as a `.txt` file and pastes that file's path instead of
    /// typing the text. `0` switches the behaviour off.
    ///
    /// Normalized here as well as at load, for the same reason
    /// `terminal_font_size` is: `set_settings` and the raw config editor can
    /// both put a fresh value in memory without going back through
    /// `load_config`, and an out-of-range number reaching the browser would
    /// turn every prompt into a file.
    ///
    /// Not published for a TERMINAL to read, because a terminal has no use for
    /// it: a long paste into a shell is a command or a heredoc, and dux never
    /// files one away. Web-only behavior, as the setting itself is.
    pub upload_pasted_text_chars: usize,
    /// Mirrors `config.ui.auto_reopen_agents`: the GLOBAL startup auto-reopen
    /// switch (default false). When on, agents that were running when dux last
    /// exited (and have their per-agent opt-in) are relaunched at startup, by
    /// the TUI and by `dux serve` alike. Read by the web's Preferences dialog.
    pub auto_reopen_agents: bool,
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
    /// (an unrecognized config value normalizes to "focused").
    ///
    /// `config.capabilities.passthrough` is resolved INTO this value rather than
    /// published beside it: the clipboard write is the only thing an agent
    /// forwards outward on this surface, so the master switch has exactly one
    /// web consequence and the browser reads one answer instead of combining
    /// two. `passthrough = false` publishes "off" here. It deliberately does not
    /// touch `web_notifications`, which is the only switch over browser desktop
    /// notifications. Older servers omit this field, so the web client falls back
    /// to "focused".
    pub clipboard_passthrough: String,
    /// Mirrors `config.ui.pr_banner_position` ("top" | "bottom"). Desktop web
    /// places the PR banner lane above the terminal when "top" and below it when
    /// "bottom", matching the TUI's `pr_banner_at_bottom` semantics. Mobile
    /// ignores this and always renders the banner on top.
    pub pr_banner_position: String,
    /// Mirrors `config.ui.agent_sort`. The web flat agent list initializes its
    /// sort control from this so a chosen order (including the manual drag order)
    /// survives restarts and is shared across clients. Older servers omit it, so
    /// the web treats a missing value as "active".
    pub agent_sort: String,
    /// Mirrors `config.ui.agent_scrollback_lines`. The web sizes each xterm.js
    /// instance's scrollback to this so it can retain the full history the
    /// reconnect repaint replays — without it, xterm.js silently caps at its
    /// 1000-line default and trims the replayed history.
    pub agent_scrollback_lines: usize,
    /// Mirrors `config.ui.show_changes_pane`. The desktop web hides the
    /// right-hand Changes pane when false. The runtime hide/show controls (the
    /// Changes actions menu's hide item, the header's show button) persist this
    /// same preference on every flip; the browser's client-side override is
    /// only an optimistic echo, dropped once the refreshed bootstrap confirms
    /// the value. Older servers omit it, so the web treats a missing value as
    /// `true`.
    pub show_changes_pane: bool,
    /// Global environment variables from `[env]` in `config.toml`, applied to
    /// every spawned provider/terminal. Surfaced so a client can pre-fill an
    /// edit dialog.
    pub global_env: std::collections::BTreeMap<String, String>,
    /// Mirrors `config.ui.status_clear_seconds`. The web honors it for toast
    /// auto-dismiss, and every tone dismisses: an info/success toast clears
    /// this many seconds after it arrives, a warning at
    /// [`WARNING_CLEAR_FACTOR`](crate::statusline::WARNING_CLEAR_FACTOR) times
    /// that and an error at four times, so this one value grades them all. 0 disables auto-clear
    /// for those final states. (A busy toast is not a final state: the web
    /// retires it on its own fixed leak guard, comfortably longer than
    /// `statusline::BUSY_TIMEOUT`.)
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
    /// Mirrors `config.ui.tab_reaches_agent`: when true the TUI's typeable
    /// center pane sends Tab and Shift-Tab to the agent instead of cycling
    /// panes. Projected only so the web's Preferences dialog can show and
    /// change it; nothing in the web UI reads it for its own behavior. Older
    /// servers omit it, so the web treats a missing value as `false`.
    pub tab_reaches_agent: bool,
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
    /// The first-run welcome screen's content, from `dux_core::welcome_screen`.
    /// Projected UNCONDITIONALLY, not only when the welcome is pending: the app
    /// menu can open the screen on demand at any time, and the content is
    /// config-static (its last paragraph interpolates this machine's config
    /// path), so the bootstrap document is exactly its home. Distinct from
    /// `welcome_tips`, which is the rotating idle-pane tip list.
    pub welcome_screen: WelcomeScreenView,
    /// `dux_core::urls::WEBSITE` — where the welcome screen's secondary button
    /// goes. Projected rather than hardcoded client-side so the surfaces cannot
    /// disagree about a dux URL (see the `urls` module docs).
    pub website_url: String,
    /// The first-load screen this launch should show, or `None` for neither.
    ///
    /// NOT engine state, and [`Engine::bootstrap`] always leaves it `None`. The
    /// web server computes the plan ONCE at startup (`first_load::plan`, then
    /// `after_fetch` when the release-notes worker returns), holds the result in
    /// memory, and injects it into this document on every request — so a browser
    /// that connects a minute after startup still receives the screen. The
    /// version is stamped as seen when the user DISMISSES it, never when the
    /// plan is computed (see the `first_load` module docs).
    pub pending_first_load: Option<PendingFirstLoadView>,
    /// Mirrors `config.ui.disable_automated_welcome_screen`: suppresses the
    /// AUTOMATIC first-run welcome only. The app menu's "Welcome screen…" still
    /// opens it. Read by the web's Preferences dialog.
    pub disable_automated_welcome_screen: bool,
    /// Mirrors `config.ui.disable_release_notes`: suppresses the AUTOMATIC
    /// what's-new screen after a version change only. The app menu's "What's
    /// new…" still opens it. Read by the web's Preferences dialog.
    pub disable_release_notes: bool,
    /// Mirrors `config.server.file_drop_max_bytes`: the per-file size cap for a
    /// file dropped onto a pane, where `0` switches the feature OFF.
    ///
    /// Projected so a browser can tell whether the feature EXISTS. The server
    /// stays the enforcement (it refuses the upload either way), but without
    /// this the pane advertised a drop target and accepted a drop for a feature
    /// that was switched off, and only then collected a refusal per file.
    ///
    /// NOT YET KNOWN IS NOT ENABLED. An older server omits the field, and the
    /// browser renders before the bootstrap document has arrived at all, so the
    /// web treats an absent value as OFF and offers nothing until dux has said
    /// the feature exists. Defaulting that window to on would advertise a drop
    /// target for a feature that may be switched off.
    pub file_drop_max_bytes: usize,
    /// Mirrors `config.server.replay_wait_seconds`: how long, in seconds of
    /// VISIBLE time, a terminal pane waits for its screen to arrive after its
    /// connection opens before it offers Reconnect.
    ///
    /// One of four timings the browser's attach state machine needs and the
    /// server never reads. They ride this document rather than a channel of
    /// their own for the same reason every other config-derived value does: a
    /// config reload refetches the bootstrap, so editing the file retimes every
    /// open tab with no restart.
    pub replay_wait_seconds: u32,
    /// Mirrors `config.server.reconnect_backoff_cap_seconds`: the longest gap
    /// the browser leaves between two automatic reconnect attempts.
    pub reconnect_backoff_cap_seconds: u32,
    /// Mirrors `config.server.heartbeat_seconds`: how often a visible page
    /// checks that its terminal connection is really alive.
    pub heartbeat_seconds: u32,
    /// Mirrors `config.server.heartbeat_deadline_seconds`: how long, in seconds
    /// of VISIBLE time, the page waits for that answer before forcing a plain
    /// reconnect.
    pub heartbeat_deadline_seconds: u32,
    /// Mirrors `config.server.tailscale` as its canonical tri-state name
    /// ("auto" | "yes" | "no"), so the Preferences row shows the saved mode. An
    /// unrecognized config value projects as "auto", which is what the serve
    /// path degrades it to.
    pub tailscale_mode: String,
    /// Whether the RUN was started with `--no-tailscale`, which outranks the
    /// saved mode for as long as it lasts.
    ///
    /// NOT engine state, and [`Engine::bootstrap`] always leaves it `false`: the
    /// flag belongs to the process that parsed the command line, so the web
    /// server injects it per request the way it injects `pending_first_load`.
    /// Without it the Preferences row would offer a mode the run will refuse and
    /// say nothing about why.
    pub tailscale_forced_no: bool,
}

/// One numbered getting-started step, projected from
/// `dux_core::welcome_screen::WelcomeStep`. The number is carried, not derived
/// from an array index, so the client never has to re-derive the sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WelcomeStepView {
    pub number: u8,
    pub title: String,
    pub detail: String,
}

/// The first-run welcome screen's content, projected from
/// `dux_core::welcome_screen::WelcomeScreen`.
///
/// Plain prose and titles: the client renders text, never Markdown. Core does
/// the trimming so neither surface needs a Markdown renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WelcomeScreenView {
    pub tagline: String,
    pub paragraphs: Vec<String>,
    pub steps: Vec<WelcomeStepView>,
}

impl WelcomeScreenView {
    /// Project the welcome content for the machine whose config lives at
    /// `config_path`.
    pub fn from_core(screen: crate::welcome_screen::WelcomeScreen) -> Self {
        Self {
            tagline: screen.tagline.to_string(),
            paragraphs: screen.paragraphs,
            steps: screen
                .steps
                .iter()
                .map(|s| WelcomeStepView {
                    number: s.number,
                    title: s.title.to_string(),
                    detail: s.detail.to_string(),
                })
                .collect(),
        }
    }
}

/// The pending first-load screen: which one, plus the release notes when the
/// screen is the what's-new one.
///
/// Deliberately minimal. The welcome screen needs no payload here because its
/// content rides [`BootstrapView::welcome_screen`] unconditionally, so this
/// carries only what is specific to THIS launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingFirstLoadView {
    /// `"welcome"` | `"whats_new"`. A string, not a bool, so a third screen can
    /// be added without reshaping the wire.
    pub screen: String,
    /// The release notes for the running version. `Some` exactly when `screen`
    /// is `"whats_new"`: the gate never shows that screen without notes in hand
    /// (a failed fetch downgrades the plan to "show nothing").
    pub notes: Option<crate::release_notes::ReleaseNotes>,
}

impl PendingFirstLoadView {
    /// The wire name for [`crate::first_load::FirstLoad::Welcome`].
    pub const WELCOME: &'static str = "welcome";
    /// The wire name for [`crate::first_load::FirstLoad::WhatsNew`].
    pub const WHATS_NEW: &'static str = "whats_new";

    pub fn welcome() -> Self {
        Self {
            screen: Self::WELCOME.to_string(),
            notes: None,
        }
    }

    pub fn whats_new(notes: crate::release_notes::ReleaseNotes) -> Self {
        Self {
            screen: Self::WHATS_NEW.to_string(),
            notes: Some(notes),
        }
    }
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
}

/// The serialized workspace of an agent: a TAGGED union, one variant per
/// [`crate::model::AgentWorkspace`] variant, mirroring [`TerminalOwnerView`]'s
/// shape exactly. Because the client receives the tag it can switch on it
/// exhaustively (see `crates/dux-web/web/src/lib/agentWorkspace.ts`, whose
/// switches end in `assertNever`) instead of inferring an agent's kind from
/// whether some string happened to be empty.
///
/// The git fields live INSIDE the managed variant, so there is no shape in
/// which a standalone agent carries a branch name at all. That is the wire
/// half of the same either/or the Rust model enforces: an empty string on the
/// wire is a lie some screen eventually renders.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentWorkspaceView {
    Managed {
        project_id: String,
        branch_name: String,
        /// The branch this agent was created on, immutable. Distinct from
        /// `branch_name` (the current branch, which tracks the worktree). When
        /// they differ the current branch has drifted since creation.
        initial_branch: String,
        /// Where the agent's branch came from ("created" | "attached" |
        /// "adopted" | "unknown"), which decides whether deleting the agent may
        /// delete the branch. The delete dialog's copy turns on it.
        branch_provenance: String,
        /// The branch this agent was forked from (its fork point / leading
        /// branch).
        source_branch: String,
        worktree_path: String,
    },
    Folder {
        /// The folder as it exists on the SERVER's filesystem.
        folder_path: String,
        /// The same folder shortened against the server's home directory, for
        /// display. Computed here because the browser may not be on the
        /// server's machine and so cannot shorten it correctly itself.
        folder_label: String,
        /// The live repository verdict for the folder: "working_repo" |
        /// "inside_repo_rooted_elsewhere" | "no_repo" | "indeterminate" |
        /// "unprobed" (nobody has looked yet; it gates exactly as
        /// "indeterminate" does, and its quiet reason reads as a wait rather
        /// than as a fault, because a freshly created agent in a healthy
        /// repository spends a moment there). The
        /// changes region renders a real repository view for the first and its
        /// quiet copy otherwise. Decided on the server so both surfaces show
        /// the same answer the server acted on.
        repo_status: String,
        /// The sentence explaining why the changes region is quiet, when it is.
        /// Carried rather than re-authored client-side so the TUI and the web
        /// say the same thing.
        quiet_reason: String,
    },
}

impl AgentWorkspaceView {
    /// Project the model workspace, folding in the live repository verdict a
    /// folder needs (the model does not carry it: it is engine state, refreshed
    /// by a probe).
    pub fn from_workspace(
        workspace: &crate::model::AgentWorkspace,
        repo_status: crate::git::FolderRepoStatus,
    ) -> Self {
        match workspace {
            crate::model::AgentWorkspace::Managed(managed) => Self::Managed {
                project_id: managed.project_id.clone(),
                branch_name: managed.branch_name.clone(),
                initial_branch: managed.initial_branch.clone(),
                branch_provenance: managed.branch_provenance.as_str().to_string(),
                source_branch: managed.source_branch.clone(),
                worktree_path: managed.worktree_path.clone(),
            },
            crate::model::AgentWorkspace::Folder(folder) => Self::Folder {
                folder_path: folder.folder_path.clone(),
                folder_label: crate::home_path::shorten_home(std::path::Path::new(
                    &folder.folder_path,
                )),
                repo_status: folder_repo_status_wire(repo_status).to_string(),
                quiet_reason: repo_status.quiet_reason().to_string(),
            },
        }
    }
}

/// The wire spelling of a [`crate::git::FolderRepoStatus`]. Its own function
/// rather than a method on the enum, because the enum lives in `git` and the
/// wire vocabulary belongs here with the rest of the projections.
fn folder_repo_status_wire(status: crate::git::FolderRepoStatus) -> &'static str {
    match status {
        crate::git::FolderRepoStatus::WorkingRepo => "working_repo",
        crate::git::FolderRepoStatus::InsideRepoRootedElsewhere => "inside_repo_rooted_elsewhere",
        crate::git::FolderRepoStatus::NoRepo => "no_repo",
        crate::git::FolderRepoStatus::Indeterminate => "indeterminate",
        crate::git::FolderRepoStatus::Unprobed => "unprobed",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub title: Option<String>,
    pub provider: String,
    /// Where this agent lives and what dux may do there. See
    /// [`AgentWorkspaceView`].
    pub workspace: AgentWorkspaceView,
    /// "active" | "detached" | "exited"
    pub status: String,
    pub auto_reopen_enabled: bool,
    /// Associated GitHub pull request, if one is tracked for this session.
    pub pr: Option<PrView>,
    /// Whether the user detached this agent's pull request, which switches
    /// autodetection off for it until they attach one by hand or resume
    /// detection explicitly. Lives on `SessionView` and not on `PrView`
    /// (unlike `overridden`) precisely because it is the state in which there
    /// is NO pull request to hang it off: it is what lets a surface offer the
    /// way back.
    pub pr_autodetect_suppressed: bool,
    /// The id of this agent's **session-slot tab**: its first tab. Closing that
    /// tab hands the slot on, so the pointer moves; what cannot be closed is an
    /// agent's only tab. Published so the browser never has to infer slot-ness
    /// from the session id (see `AgentSession::slot_tab_id`); every slot-ness
    /// decision in the web client reads this field.
    pub slot_tab_id: String,
    /// Provider tabs for this session: the session-slot tab (`tabs[0]`, the one
    /// named by `slot_tab_id`) first, then extra tabs in creation order. Always
    /// non-empty. The client shows the tab strip only when `tabs.len() >= 2`.
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
    /// Whether the user is currently typing into any of this agent's tabs (a
    /// keystroke landed within [`crate::engine::AGENT_INPUT_SUPPRESSION_WINDOW`]).
    /// Rolled up any-tab like `working`, and deliberately disjoint from it:
    /// `working` already excludes typing (via [`crate::engine::Engine::is_agent_streaming`]),
    /// so a tab is at most one of typing/working at a time.
    pub typing: bool,
    /// Whether any of this agent's tabs currently needs attention (a permission
    /// prompt, a finished turn) that the user has not yet looked at. Rolled up
    /// any-tab, mirroring `working`. Memory-only runtime state; the web surfaces
    /// this as a sidebar dot, a browser-tab count, and a favicon dot.
    pub needs_attention: bool,
    /// Session creation time as an RFC 3339 / ISO 8601 string. Exposed so both
    /// surfaces can compute the same "recently created" display order over the
    /// shared `config.ui.agent_sort` mode, so a sort set on either stays in sync
    /// by construction.
    pub created_at: String,
    /// Session last-update time as an RFC 3339 / ISO 8601 string. Mirror of
    /// `created_at`; backs the shared "recently updated" display sort.
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

/// The serialized owner of a terminal: a TAGGED union, one variant per
/// [`crate::model::TerminalOwner`] variant, carrying the owner's id. This is the
/// wire counterpart of the owner methods on `TerminalOwner`: because the client
/// receives the tag, it can switch on it exhaustively (see
/// `crates/dux-web/web/src/lib/terminalOwner.ts`, whose switches end in
/// `assertNever`) instead of inferring ownership from which collection a
/// terminal happened to be nested in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalOwnerView {
    /// A companion terminal spawned in an agent's worktree.
    Session { session_id: String },
    /// A project terminal spawned at a project's repo root, with no agent.
    Project { project_id: String },
    /// A standalone terminal, belonging to no agent and no project. There is no
    /// owner id to send, so it carries the thing its row names it by instead:
    /// the directory it opened in, written with the home directory collapsed to
    /// `~` ([`crate::home_path::shorten_home`]). That string is the row's second
    /// line and is what the sidebar search matches. It travels on the wire
    /// rather than being derived client-side because the browser is not
    /// necessarily on the same machine as the server and has no home directory
    /// of the server's to collapse against.
    Standalone { cwd_label: String },
}

impl crate::model::TerminalOwner {
    /// Presentation: project the owner onto the wire. Exhaustive, so a new owner
    /// kind cannot reach the browser as an untagged or missing owner.
    ///
    /// `cwd` is the directory the terminal was spawned in
    /// ([`crate::pty::PtyClient::spawn_dir`]); only the standalone kind uses it,
    /// because only it names itself by where it is. Deliberately the SPAWN
    /// directory rather than a live probe: this string is projected on every
    /// spine build and feeds the coarse change fingerprint, so a value that
    /// moved every time the user typed `cd` would churn the sidebar and make the
    /// row's search key unstable.
    pub fn to_view(&self, cwd: &std::path::Path) -> TerminalOwnerView {
        match self.as_ref() {
            crate::model::TerminalOwnerRef::Session(id) => TerminalOwnerView::Session {
                session_id: id.to_string(),
            },
            crate::model::TerminalOwnerRef::Project(id) => TerminalOwnerView::Project {
                project_id: id.to_string(),
            },
            crate::model::TerminalOwnerRef::Standalone => TerminalOwnerView::Standalone {
                cwd_label: crate::home_path::shorten_home(cwd),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalView {
    pub id: String,
    /// Who this terminal belongs to, tagged. Every terminal carries its own
    /// owner now that they arrive as one flat collection rather than nested
    /// under the session or project that owns them.
    pub owner: TerminalOwnerView,
    pub label: String,
    /// Whether the terminal's PTY has emitted any output yet.
    pub has_output: bool,
    /// Whether this terminal is busy ([`crate::engine::Engine::terminal_is_working`]):
    /// it is streaming output right now, OR a foreground app is running in it even
    /// while quiet (a set `foreground_cmd`, i.e. the shell no longer owns the
    /// terminal foreground). Typing takes precedence, so this excludes the echo of
    /// the user's own typing. Terminals have no detached or needs-attention notion,
    /// so those fields are deliberately absent here.
    pub working: bool,
    /// Whether the user is currently typing into this terminal (a keystroke
    /// landed within [`crate::engine::AGENT_INPUT_SUPPRESSION_WINDOW`]). Disjoint
    /// from `working`, which excludes typing.
    pub typing: bool,
    /// The command currently running in the foreground of this terminal, or
    /// `None` when the shell itself is idle in the foreground. Projected verbatim
    /// from [`crate::model::CompanionTerminal::foreground_cmd`], which the engine
    /// refreshes at most every ~2s
    /// ([`crate::engine::FOREGROUND_REFRESH_INTERVAL`]) — so this field changes
    /// slowly and the coarse `sessions.changed` signal stays calm. The web UI
    /// shows this as the terminal's title when present, falling back to `label`.
    pub foreground_cmd: Option<String>,
    /// The terminal's manual (drag) position within the flat Terminals section,
    /// ascending. Projected verbatim from
    /// [`crate::model::CompanionTerminal::sort_order`]; the base order both
    /// surfaces render before applying the active sort mode on top. Stamped at
    /// spawn from a monotonic counter (so it defaults to creation order) and
    /// rewritten only by a reorder. Runtime-only, like the terminal itself.
    pub sort_order: u64,
    /// Terminal spawn time as an RFC 3339 / ISO 8601 string. Same representation
    /// as [`SessionView::created_at`], so both surfaces can compute the same
    /// "recently created" order across terminals and agents. Immutable.
    pub created_at: String,
    /// Terminal last-activity time as an RFC 3339 / ISO 8601 string: the wall
    /// clock of the terminal's most recent PTY activity
    /// ([`crate::engine::Engine::pty_activity`]), falling back to `created_at`
    /// when the terminal has not emitted or received anything yet. Backs the
    /// shared "recently updated" display sort; mirror of [`SessionView::updated_at`].
    pub updated_at: String,
    /// The connection id currently holding input+sizing ownership of this
    /// terminal's PTY, or `None` when nobody drives it.
    ///
    /// The exact mirror of [`AgentTabView::input_owner`], for the same reason
    /// and with the same wire shape: ownership lives in the web layer's
    /// registry, not in the engine, so the engine projects `None` here and the
    /// web layer's spine overlay stamps the real value. Published as a STRING
    /// so it compares directly against the `owner` field of the `pty.owner`
    /// handover frames and the PTY handshake, which is what a client matches
    /// against its own PTY-socket ids.
    ///
    /// A terminal pane's take-over card reads it to tell a stale driver name
    /// from a fresh one, which is why it is published at all: for a while it
    /// deliberately was not, because nothing consumed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_owner: Option<String>,
}

/// One provider tab of an agent, projected for the tab strip. `order == 0` is
/// the **session-slot tab**, the one named by the session's stored
/// `SessionView::slot_tab_id` pointer. It is not privileged for CLOSING (closing
/// it hands the slot to the next tab in strip order) and no tab is privileged
/// for RESUME: that is decided per provider by liveness at launch
/// (see `Engine::tab_resume_decision`), not by position.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentTabView {
    /// Tab id. Matches `SessionView::slot_tab_id` for the session-slot tab.
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
    /// Whether the user is currently typing into this tab (a keystroke landed
    /// within [`crate::engine::AGENT_INPUT_SUPPRESSION_WINDOW`]). Disjoint from
    /// `working`, which excludes typing.
    pub typing: bool,
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
    /// Whether this tab's LAST run ended badly: a launch that failed, or a
    /// process that exited non-zero. Only meaningful while `has_live_process` is
    /// `false`, and it is what tells a surface apart the two kinds of dormant
    /// tab: one that is simply not running yet (a restart, a stop) and one that
    /// tried and failed. The web starts the first on selection and shows the
    /// second its diagnosis card instead, so a tab that keeps failing cannot
    /// relaunch itself every time the user looks at it.
    ///
    /// Uniform across every tab; no slot special-casing lives in the data.
    /// Memory-only, so a restart clears it (see [`crate::engine::Engine::failed_tab_runs`]).
    pub last_run_failed: bool,
    /// What this tab's LIVE process launched with, for a file dropped onto its
    /// pane; `None` when no process is live.
    ///
    /// It rides the SPINE rather than the bootstrap document, and that is the
    /// whole point of where it lives. It changes when a process LAUNCHES or
    /// TERMINATES, and the spine is what a launch and a termination refresh
    /// (`sessions.changed`); the bootstrap document is refreshed by
    /// `config.changed`, which is a different event entirely. Published there it
    /// went stale in the browser for the whole life of a process: a client that
    /// had refetched config before a relaunch kept resolving the OLD entry, so a
    /// tab relaunched under a different provider was still quoted for the
    /// previous one until a reconnect or a restart.
    ///
    /// Per TAB rather than per provider name because a provider name cannot carry
    /// the answer: launch a tab, edit `[providers.<name>] web_dragdrop_paste`,
    /// launch a second tab, and both processes report the same provider name
    /// while each needs the form it started with. It also answers after the user
    /// renames or removes that block while the tab is still running, which the
    /// config-keyed map cannot.
    ///
    /// The browser resolves a pane as: this, then
    /// [`BootstrapView::provider_drop_paste`] by provider name, then the
    /// defaults. So the LAUNCHED profile wins for a live tab and a config edit
    /// takes effect on that tab's next launch.
    ///
    /// Retired with the process by [`crate::engine::Engine::clear_tab_runtime`].
    pub drop_paste: Option<DropPasteView>,
    /// The ownership-registry participant id that currently owns input for this
    /// tab's PTY, or `None` when nobody does.
    ///
    /// The ENGINE never fills this field. The shared registry lives in
    /// [`crate::pty_owners`], and the serving layer overlays its live snapshot
    /// onto every view it serves: the `/spine` document
    /// (overlaid before fingerprinting, so an ownership flip fires
    /// `sessions.changed` like any other spine change) and the
    /// projects/sessions list and single-session REST reads (overlaid in their
    /// request arms). A `SessionView` obtained from the engine DIRECTLY (the
    /// TUI, or a dux-core test) always has `None` here.
    ///
    /// The value is the owning registry connection id, stringified: either a
    /// PTY socket's id or the background-serving TUI's seat. It is NOT the
    /// events-socket `X-Connection-Id` UUID. Publishing the
    /// identity rather than a per-client "elsewhere" boolean keeps the spine
    /// one shared document: each client compares the id against its own live
    /// PTY-socket ids and decides "owned, and not by me" locally, which is how
    /// the agent list's row menus can disable mutating actions for an agent
    /// another device is driving without this client ever attaching to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_owner: Option<String>,
}

/// Everything the browser needs to write a DROPPED file's path into one pane:
/// the form the path takes, and the CLI that will read it.
///
/// The two are published together, never separately, because they answer to the
/// same question (which CLI is on the other end of this paste) and mixing sources
/// would describe a CLI that is not there. A pane resolves ONE of these and reads
/// both halves out of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DropPasteView {
    /// The paste form, normalized to a name [`crate::config::WebDragDropPaste`]
    /// recognizes (a misspelled config value resolves to `"bare"` here, having
    /// already been warned about once at load).
    pub form: String,
    /// The FILE NAME of the command being run, which is what identifies the CLI
    /// receiving the paste, and which the browser keys its measured per-CLI
    /// paste-length table by.
    ///
    /// The provider's BLOCK NAME is deliberately not what travels here. A
    /// provider's name and the command it runs are independent, so
    /// `[providers.myagent] command = "codex"` is a real Codex and
    /// `[providers.codex] command = "something-else"` is not. Keying the table by
    /// the name (which it was) was wrong in both directions at once: a real Codex
    /// under any other name got no limit and was handed oversized paths it
    /// silently ignores, and an unrelated CLI merely named codex had valid long
    /// paths withheld from it. See
    /// [`crate::config::ProviderCommandConfig::command_file_name`] for why it is
    /// the file name and not the whole string.
    pub command_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrView {
    pub number: u64,
    /// "open" | "merged" | "closed"
    pub state: String,
    pub title: String,
    pub url: String,
    /// Whether this PR was manually attached (pinned) rather than autodetected:
    /// plain provenance. On `PrView` and NOT on `SessionView`, so "overridden
    /// without a PR" is unrepresentable. Detach itself no longer needs this
    /// flag to propagate; it removes the PR from the view in the same spine
    /// change.
    pub overridden: bool,
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
    #[allow(clippy::too_many_arguments)]
    fn from_session(
        s: &AgentSession,
        pr: Option<&PrInfo>,
        pr_overridden: bool,
        pr_autodetect_suppressed: bool,
        tabs: Vec<AgentTabView>,
        has_output: bool,
        working: bool,
        typing: bool,
        needs_attention: bool,
        repo_status: crate::git::FolderRepoStatus,
    ) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            provider: s.provider.as_str().to_string(),
            workspace: AgentWorkspaceView::from_workspace(&s.workspace, repo_status),
            status: s.status.as_str().to_string(),
            auto_reopen_enabled: s.auto_reopen_enabled,
            pr: pr.map(|pr| PrView::from_pr(pr, pr_overridden)),
            pr_autodetect_suppressed,
            slot_tab_id: s.slot_tab_id().to_string(),
            tabs,
            has_output,
            working,
            typing,
            needs_attention,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            last_focused_tab: s.last_focused_tab.clone(),
        }
    }
}

impl PrView {
    fn from_pr(pr: &PrInfo, overridden: bool) -> Self {
        Self {
            overridden,
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
    /// Project the projects/sessions/sidebar spine served via
    /// `GET /api/v1/workspace`, pushed on `/ws/events`, and read by the thin
    /// per-resource endpoints. This is the exact projection logic the
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
                .map(ProjectView::from_project)
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
            terminals: self.terminal_views(),
            sidebar: crate::sidebar::build_sidebar(
                &self.projects,
                &self.sessions,
                &self.project_ids_with_terminals(),
                self.config.ui.empty_project_separator_min_projects,
            ),
        }
    }

    /// Project one companion terminal into its [`TerminalView`]. Shared by both
    /// build sites so their field set (and the `updated_at` derivation) can never
    /// drift.
    fn terminal_view(&self, id: &str, t: &crate::model::CompanionTerminal) -> TerminalView {
        TerminalView {
            id: id.to_string(),
            owner: t.owner.to_view(t.client.spawn_dir()),
            // Always `None` here: PTY ownership lives in the web layer's
            // registry, so the spine overlay stamps it. See the field's doc.
            input_owner: None,
            label: t.label.clone(),
            has_output: t.client.has_output(),
            // A terminal is Working when it is streaming output OR a foreground app
            // is running in it (see `terminal_is_working`); typing takes precedence.
            working: self.terminal_is_working(id),
            typing: self.is_typing(id),
            foreground_cmd: t.foreground_cmd.clone(),
            sort_order: t.sort_order,
            created_at: t.created_at.to_rfc3339(),
            // `pty_activity` records the last activity as a monotonic `Instant`;
            // map it back onto wall clock (now minus how long ago it fired) to
            // match `created_at`'s RFC 3339 representation. No activity yet →
            // fall back to the spawn time.
            updated_at: self
                .pty_activity
                .get(id)
                .map(|last| {
                    let ago = chrono::Duration::from_std(last.elapsed()).unwrap_or_default();
                    (chrono::Utc::now() - ago).to_rfc3339()
                })
                .unwrap_or_else(|| t.created_at.to_rfc3339()),
        }
    }

    /// EVERY companion terminal as one flat, owner-bearing [`TerminalView`] list,
    /// ordered by the manual `sort_order`. The single terminal projection: there
    /// is no per-owner variant to forget to extend.
    ///
    /// The order is the same global `sort_order` ascending the two nested
    /// projections produced within their own groups, and both surfaces re-apply
    /// the active sort mode over this base, so flattening changes nothing the
    /// user sees.
    fn terminal_views(&self) -> Vec<TerminalView> {
        let mut terminals: Vec<TerminalView> = self
            .companion_terminals
            .iter()
            .map(|(id, t)| self.terminal_view(id, t))
            .collect();
        terminals.sort_by_key(|a| a.sort_order);
        terminals
    }

    /// Every terminal owned by `owner`, in the same `sort_order` ascending order
    /// as the flat [`Engine::terminal_views`].
    ///
    /// The flat, owner-tagged collection is what the BROWSER receives. The thin
    /// REST reads (`GET /api/v1/sessions/:id`, `/api/v1/sessions`,
    /// `/api/v1/projects`) are a separately documented programmability surface
    /// that has always nested a terminal inside its owner, and they re-nest
    /// through this rather than making every script reading them move to the
    /// spine document. Public for that reason.
    pub fn terminal_views_for_owner(
        &self,
        owner: crate::model::TerminalOwnerRef<'_>,
    ) -> Vec<TerminalView> {
        let mut terminals: Vec<TerminalView> = self
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.owner.as_ref() == owner)
            .map(|(id, t)| self.terminal_view(id, t))
            .collect();
        terminals.sort_by_key(|a| a.sort_order);
        terminals
    }

    /// The set of project ids that own at least one live project terminal. Feeds
    /// the sidebar split so a project with a live terminal never sinks below the
    /// "no agents" separator.
    pub fn project_ids_with_terminals(&self) -> std::collections::HashSet<String> {
        self.companion_terminals
            .values()
            .filter_map(|t| match t.owner.as_ref() {
                crate::model::TerminalOwnerRef::Project(pid) => Some(pid.to_string()),
                // Neither a session terminal nor a standalone terminal keeps a
                // project row above the "no agents" separator: the first belongs
                // to an agent the project already lists, and the second belongs
                // to no project at all.
                crate::model::TerminalOwnerRef::Session(_)
                | crate::model::TerminalOwnerRef::Standalone => None,
            })
            .collect()
    }

    /// Project a single session into its [`SessionView`], looking up its PR
    /// status, output, and streaming flag exactly as [`Engine::spine`] does, so
    /// the per-session REST read (`GET /api/v1/sessions/:id`) can project ONLY
    /// the requested session instead of building the whole spine.
    ///
    /// A session does not carry its terminals: they live in the spine's one
    /// flat [`SpineView::terminals`] collection, each tagged with its owner.
    fn project_session(
        &self,
        s: &AgentSession,
        support_tabs: &[&crate::model::AgentTab],
    ) -> SessionView {
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
        let has_output = std::iter::once(s.slot_tab_id())
            .chain(support_tabs.iter().map(|t| TabIdRef::new(&t.id)))
            .any(|id| self.providers.get(id).is_some_and(|p| p.has_output()));
        let working = std::iter::once(s.slot_tab_id())
            .chain(support_tabs.iter().map(|t| TabIdRef::new(&t.id)))
            .any(|id| self.is_agent_streaming(id.as_str()));
        // Typing rolls up any-tab too, and is disjoint from `working` (which
        // excludes typing). Uses the shared any-tab rollup so the sidebar row and
        // the per-session read agree.
        let typing = self.session_is_typing(&s.id);
        // Attention rolls up any-tab, exactly like `working`: the sidebar row
        // marks the agent if any of its tabs (session-slot or extra) is flagged.
        let needs_attention = std::iter::once(s.slot_tab_id())
            .chain(support_tabs.iter().map(|t| TabIdRef::new(&t.id)))
            .any(|id| self.tab_needs_attention(id.as_str()));
        // Tabs, session-slot first, then extras in creation order.
        let mut tabs = vec![self.tab_view(s.slot_tab_id(), self.running_provider_for(s), 0)];
        let mut support: Vec<_> = support_tabs.to_vec();
        support.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        for (i, t) in support.into_iter().enumerate() {
            let effective = self
                .running_provider_pins
                .get(TabIdRef::new(&t.id))
                .cloned()
                .unwrap_or_else(|| t.provider.clone());
            tabs.push(self.tab_view(TabIdRef::new(&t.id), effective, (i + 1) as u32));
        }
        SessionView::from_session(
            s,
            self.pr_statuses.get(&s.id),
            self.pr_overrides.contains_key(&s.id),
            self.pr_suppressions.contains(&s.id),
            tabs,
            has_output,
            working,
            typing,
            needs_attention,
            self.folder_repo_status(&s.id),
        )
    }

    /// Project one tab id into an [`AgentTabView`] from the tab-keyed runtime maps.
    /// `order` is display position only; no tab is privileged.
    fn tab_view(&self, id: &TabIdRef, provider: ProviderKind, order: u32) -> AgentTabView {
        AgentTabView {
            // The wire keeps plain strings: an id leaving the engine is bytes a
            // browser stores, not an id dux is still reasoning about.
            id: id.as_str().to_string(),
            provider: provider.as_str().to_string(),
            order,
            working: self.is_agent_streaming(id.as_str()),
            typing: self.is_typing(id.as_str()),
            needs_attention: self.tab_needs_attention(id.as_str()),
            has_output: self
                .providers
                .get(id)
                .map(|p| p.has_output())
                .unwrap_or(false),
            has_live_process: self.providers.contains_key(id),
            last_run_failed: self.tab_last_run_failed(id.as_str()),
            // Read off the LIVE process's launch, so it appears when the tab
            // launches and disappears when it is torn down. Both halves come out
            // of the one recorded entry; neither is topped up from current
            // config, because a live process is not affected by an edit made
            // after it started.
            drop_paste: self.launched_drop_paste.get(id).map(|l| DropPasteView {
                form: l.form.as_str().to_string(),
                command_name: l.command_name.clone(),
            }),
            // Always `None` from the engine: input ownership lives in the web
            // layer's per-PTY-socket registry, which overlays it onto the built
            // spine (see the field doc).
            input_owner: None,
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
        // What CONFIG says, and nothing else. A live process never writes here:
        // what a tab launched with is published on the SPINE, per tab, because
        // that is what a launch and a termination refresh. Folding launched
        // entries in here collapsed a per-tab answer onto a per-provider key
        // (two sibling tabs of one provider could not both be right) AND left
        // the browser's copy stale for the life of the process.
        let provider_drop_paste: BTreeMap<String, DropPasteView> = self
            .config
            .providers
            .commands
            .iter()
            .map(|(name, provider)| {
                (
                    name.clone(),
                    DropPasteView {
                        form: provider.resolved_web_dragdrop_paste().as_str().to_string(),
                        command_name: provider.command_file_name(),
                    },
                )
            })
            .collect();
        BootstrapView {
            available_providers,
            provider_drop_paste,
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
            copy_uncommitted_changes_by_default: self
                .config
                .defaults
                .copy_uncommitted_changes_by_default,
            gh_available: self.pr_agent_command_available(),
            github_integration: self.config.ui.github_integration,
            copy_on_select: self.config.ui.copy_on_select,
            terminal_font_family: self.config.ui.terminal_font_family.clone(),
            terminal_font_size: crate::config::normalized_terminal_font_size(
                self.config.ui.terminal_font_size,
            ),
            // Normalized rather than passed through: `set_settings` and the raw
            // config editor both put a value in memory without going back
            // through `load_config`, so this is the last place a typo can be
            // caught before the browser has to act on it (the
            // `upload_pasted_text_chars` precedent).
            compose_bar: crate::config::ComposeBarMode::from_config_str(
                &self.config.ui.compose_bar,
            )
            .as_str()
            .to_string(),
            mobile_top_bar: self.config.ui.mobile_top_bar,
            mobile_accessory_bar: self.config.ui.mobile_accessory_bar,
            upload_write_gitignore: self.config.ui.upload_write_gitignore,
            upload_pasted_text_chars: crate::config::normalized_upload_pasted_text_chars(
                self.config.ui.upload_pasted_text_chars,
            ),
            auto_reopen_agents: self.config.ui.auto_reopen_agents,
            attention_grace_seconds: self.config.ui.attention_grace_seconds,
            web_notifications: self.config.capabilities.web_notifications,
            hyperlinks: self.config.capabilities.hyperlinks,
            // Resolve the `passthrough` master switch here rather than publishing
            // it beside this field: the clipboard write is the only thing an agent
            // forwards outward on the web, so the switch has one web consequence
            // and the browser gets one answer instead of two to combine. Browser
            // notifications are NOT one of its consequences; `web_notifications`
            // is the only switch over those and is published untouched above.
            clipboard_passthrough: if self.config.capabilities.passthrough {
                crate::config::ClipboardPassthroughMode::parse(
                    &self.config.capabilities.clipboard_passthrough,
                )
                .unwrap_or(crate::config::ClipboardPassthroughMode::Focused)
            } else {
                crate::config::ClipboardPassthroughMode::Off
            }
            .as_str()
            .to_string(),
            pr_banner_position: self.config.ui.pr_banner_position.clone(),
            tailscale_mode: self.config.server.tailscale_mode().as_str().to_string(),
            // Always `false` here: `--no-tailscale` is the serving process's own
            // fact, and the bootstrap ROUTE injects it. See the field's doc.
            tailscale_forced_no: false,
            agent_sort: self.config.ui.agent_sort.clone(),
            agent_scrollback_lines: self.config.ui.agent_scrollback_lines,
            show_changes_pane: self.config.ui.show_changes_pane,
            global_env: self.config.env.clone(),
            status_clear_seconds: self.config.ui.status_clear_seconds,
            title: self.config.server.title.clone(),
            favicon: self.config.server.favicon.clone(),
            agent_tabs_max: self.agent_tabs_max(),
            always_show_tab_strip: self.config.ui.always_show_tab_strip,
            tab_reaches_agent: self.config.ui.tab_reaches_agent,
            attention_indicator: self.config.ui.attention_indicator,
            attention_on_bell: self.config.ui.attention_on_bell,
            global_default_provider: self.config.defaults.provider.clone(),
            welcome_screen: WelcomeScreenView::from_core(crate::welcome_screen::welcome_screen(
                &self.paths.config_path,
            )),
            website_url: crate::urls::WEBSITE.to_string(),
            // Always `None` here: the pending screen lives in the web server's
            // memory, not the engine's, and the bootstrap ROUTE injects it. See
            // the field's doc.
            pending_first_load: None,
            disable_automated_welcome_screen: self.config.ui.disable_automated_welcome_screen,
            disable_release_notes: self.config.ui.disable_release_notes,
            file_drop_max_bytes: self.config.server.file_drop_max_bytes,
            replay_wait_seconds: self.config.server.replay_wait_seconds,
            reconnect_backoff_cap_seconds: self.config.server.reconnect_backoff_cap_seconds,
            heartbeat_seconds: self.config.server.heartbeat_seconds,
            heartbeat_deadline_seconds: self.config.server.heartbeat_deadline_seconds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::ids::SessionIdRef;
    use crate::ids::TabId;

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
        assert_eq!(
            spine.sessions[0].workspace,
            AgentWorkspaceView::Managed {
                project_id: "p1".to_string(),
                branch_name: "feature".to_string(),
                initial_branch: "feature".to_string(),
                branch_provenance: "created".to_string(),
                source_branch: "main".to_string(),
                worktree_path: "/tmp/s1-worktree".to_string(),
            }
        );
        assert_eq!(spine.sessions[0].status, "detached");
    }

    /// `overridden` lives on `PrView` (so "overridden without a PR" is
    /// unrepresentable) and flips with the engine's `pr_overrides` map, which
    /// is also what makes a detach observable to the web's sessions fingerprint
    /// before the PR data itself changes.
    #[test]
    fn session_pr_view_carries_the_overridden_flag() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        engine.pr_statuses.insert(
            "s1".to_string(),
            crate::model::PrInfo {
                number: 12,
                state: crate::model::PrState::Open,
                title: "Pinned".to_string(),
                host: "github.com".to_string(),
                owner_repo: "fork/r".to_string(),
                url: "https://github.com/fork/r/pull/12".to_string(),
            },
        );

        let pr = engine.spine().sessions[0].pr.clone().expect("pr view");
        assert!(!pr.overridden, "autodetected PRs are not overridden");

        engine.pr_overrides.insert(
            "s1".to_string(),
            crate::storage::StoredPr {
                session_id: "s1".to_string(),
                pr_number: 12,
                host: "github.com".to_string(),
                owner_repo: "fork/r".to_string(),
                state: "OPEN".to_string(),
                title: "Pinned".to_string(),
                url: "https://github.com/fork/r/pull/12".to_string(),
            },
        );
        let pr = engine.spine().sessions[0].pr.clone().expect("pr view");
        assert!(pr.overridden, "a pinned PR reports overridden");
    }

    /// The detach flag rides `SessionView`, not `PrView`: the state it
    /// describes is precisely the one with no PR, and it is what lets a
    /// surface offer "resume autodetection" only where that means something.
    #[test]
    fn session_view_reports_a_detached_agents_suppressed_autodetection() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        assert!(!engine.spine().sessions[0].pr_autodetect_suppressed);

        engine.pr_suppressions.insert("s1".to_string());
        let view = &engine.spine().sessions[0];
        assert!(view.pr_autodetect_suppressed);
        assert!(
            view.pr.is_none(),
            "a detached agent shows no PR to hang the flag off"
        );
    }

    #[test]
    fn session_view_exposes_initial_and_source_branch() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut s = sample_session("s1", "p1", "cur");
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "orig".into();
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .source_branch = "main".into();
        engine.sessions.push(s);

        let view = &engine.spine().sessions[0];
        let AgentWorkspaceView::Managed {
            branch_name,
            initial_branch,
            source_branch,
            ..
        } = &view.workspace
        else {
            panic!("a managed agent must project the managed variant");
        };
        assert_eq!(branch_name, "cur");
        assert_eq!(initial_branch, "orig");
        assert_eq!(source_branch, "main");
    }

    #[test]
    fn session_view_carries_branch_provenance_for_the_delete_dialog() {
        // The browser's delete confirm has to say whether the branch goes or
        // stays, so the projection has to tell it which kind of agent this is.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut s = sample_session("s1", "p1", "develop");
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::AttachedExisting;
        engine.sessions.push(s);

        let view = &engine.spine().sessions[0];
        let json = serde_json::to_value(view).expect("serialize");
        assert_eq!(json["workspace"]["kind"], "managed");
        assert_eq!(json["workspace"]["branch_provenance"], "attached");
    }

    /// The wire half of the either/or: a standalone agent's payload carries a
    /// folder and no branch fields AT ALL, so there is no empty string on the
    /// wire for a client to mistake for a branch. The tag is what the browser
    /// switches on, mirroring `TerminalOwnerView`.
    #[test]
    fn a_standalone_agent_projects_a_folder_workspace_with_no_git_fields() {
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(crate::engine::test_support::sample_standalone_session(
                "sa1",
                "/home/someone/notes",
            ));

        let view = &engine.spine().sessions[0];
        let json = serde_json::to_value(view).expect("serialize");
        assert_eq!(json["workspace"]["kind"], "folder");
        assert_eq!(json["workspace"]["folder_path"], "/home/someone/notes");
        for absent in [
            "branch_name",
            "initial_branch",
            "source_branch",
            "branch_provenance",
            "project_id",
            "worktree_path",
        ] {
            assert!(
                json["workspace"][absent].is_null(),
                "{absent} must not exist on a folder workspace, got {json}"
            );
        }
        // An unprobed folder is honest about not knowing yet, and its quiet
        // sentence travels with it so both surfaces say the same thing.
        assert_eq!(json["workspace"]["repo_status"], "unprobed");
        assert!(
            json["workspace"]["quiet_reason"]
                .as_str()
                .expect("a quiet reason")
                .contains("still looking")
        );
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
        // Held because the reload stores this config's `logging.level` into the
        // process-wide threshold another test may be asserting on.
        let _guard = crate::logger::level_test_guard();
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

    /// Stand up a session `s1` whose worktree exists on disk and whose terminal
    /// command is `cat`, so `create_companion_terminal` can spawn real terminals.
    /// Returns the engine and the tempdir (kept alive by the caller).
    fn engine_with_spawnable_terminals() -> (Engine, tempfile::TempDir) {
        let (mut engine, _cfg) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        (engine, worktree)
    }

    /// A companion terminal publishes its PTY's input owner exactly the way an
    /// agent tab does, and omits the key entirely when nobody drives it.
    ///
    /// The browser needs it so a terminal's take-over card can tell a stale
    /// driver name from a fresh one. The engine cannot fill the field itself
    /// (ownership lives in the web layer's registry), so the projection starts
    /// it empty and the actor's overlay stamps it.
    #[test]
    fn a_terminal_view_carries_an_input_owner_that_is_omitted_while_unowned() {
        let (mut engine, _worktree) = engine_with_spawnable_terminals();
        let (id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("spawn a terminal");

        let mut spine = engine.spine();
        let terminal = spine
            .terminals
            .iter_mut()
            .find(|t| t.id == id)
            .expect("the spawned terminal is projected");
        assert_eq!(
            terminal.input_owner, None,
            "the engine projection must leave ownership to the web layer's overlay"
        );
        let json = serde_json::to_string(&terminal).expect("serialize");
        assert!(
            !json.contains("input_owner"),
            "an undriven terminal must publish no owner at all (absent, not null): {json}"
        );

        terminal.input_owner = Some("42".to_string());
        let json = serde_json::to_string(&terminal).expect("serialize");
        assert!(
            json.contains(r#""input_owner":"42""#),
            "a driven terminal must publish the owning connection id: {json}"
        );
    }

    #[test]
    fn terminals_are_ordered_by_sort_order_and_reflect_a_reorder() {
        let (mut engine, _worktree) = engine_with_spawnable_terminals();

        let (t1, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("term 1");
        let (t2, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("term 2");
        let (t3, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("term 3");

        // Base order is creation order (ascending sort_order stamped at spawn).
        let order: Vec<String> = engine
            .spine()
            .terminals
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(order, vec![t1.clone(), t2.clone(), t3.clone()]);

        // A reorder rewrites the base order the viewmodel emits.
        engine
            .apply(crate::engine::Command::ReorderTerminals {
                terminal_ids: vec![t3.clone(), t1.clone(), t2.clone()],
            })
            .expect("reorder");

        let reordered: Vec<String> = engine
            .spine()
            .terminals
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(reordered, vec![t3, t1, t2]);
    }

    #[test]
    fn terminal_updated_at_reflects_pty_activity_else_created_at() {
        use std::time::Instant;

        let (mut engine, _worktree) = engine_with_spawnable_terminals();
        let (tid, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("term");

        // No activity yet: updated_at falls back to created_at exactly.
        let view = engine.spine().terminals[0].clone();
        assert_eq!(view.id, tid);
        assert_eq!(
            view.updated_at, view.created_at,
            "with no PTY activity, updated_at mirrors created_at"
        );

        // Stamp fresh activity: updated_at now tracks the activity, at or after
        // the spawn time.
        engine.pty_activity.insert(tid.clone(), Instant::now());
        let after = engine.spine().terminals[0].clone();
        assert_ne!(
            after.updated_at, after.created_at,
            "fresh PTY activity moves updated_at off created_at"
        );
        let created = chrono::DateTime::parse_from_rfc3339(&after.created_at).unwrap();
        let updated = chrono::DateTime::parse_from_rfc3339(&after.updated_at).unwrap();
        assert!(
            updated >= created,
            "updated_at ({updated}) must be at or after created_at ({created})"
        );
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
        engine
            .pty_activity
            .insert("s1-slot".to_string(), Instant::now());

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
            TabId::new("t1"),
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
        assert_eq!(session.tabs[0].id, "s1-slot");
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
        assert_eq!(vm.sessions[0].tabs[0].id, "s1-slot");
        assert_eq!(vm.sessions[0].tabs[0].order, 0);
    }

    #[test]
    fn the_session_view_publishes_which_tab_is_the_slot_tab() {
        // The browser reads `slot_tab_id` instead of comparing a tab id against
        // the session id, so the wire has to carry it, and `tabs[0]` has to be
        // the tab it names. TWIN of the web's `isFirstTab` cases in
        // `lib/agentTabs.test.ts`; keep the two in lockstep.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        engine.agent_tabs.insert(
            TabId::new("tab-b"),
            crate::model::AgentTab {
                id: "tab-b".to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );

        let vm = engine.spine();
        let session = &vm.sessions[0];
        assert_eq!(
            session.slot_tab_id,
            engine.slot_tab_id_of(SessionIdRef::new("s1")).as_str()
        );
        assert_eq!(session.tabs[0].id, session.slot_tab_id);
        assert_ne!(session.tabs[1].id, session.slot_tab_id);
    }

    #[test]
    fn the_published_slot_tab_moves_with_a_promotion() {
        // What a browser sees after the first tab is closed: the pointer names
        // the promoted tab, that tab leads the strip, and it appears exactly
        // once (it is the slot now, so it is no longer an extra as well).
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let session = sample_session("s1", "p1", "feature");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        for (id, order) in [("tab-b", 1), ("tab-c", 2)] {
            let tab = crate::model::AgentTab {
                id: id.to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: order,
                created_at: chrono::Utc::now(),
            };
            engine.session_store.insert_agent_tab(&tab).unwrap();
            engine.agent_tabs.insert(TabId::new(id), tab);
        }

        engine.close_tab("s1", "s1-slot").expect("promotion");

        let vm = engine.spine();
        let session = &vm.sessions[0];
        assert_eq!(session.slot_tab_id, "tab-b");
        assert_eq!(
            session
                .tabs
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tab-b", "tab-c"]
        );
        assert_eq!(session.tabs[0].order, 0);
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        let vm = engine.spine();
        let terminals = &vm.terminals;
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0].owner,
            TerminalOwnerView::Session {
                session_id: "s1".to_string()
            }
        );
        assert_eq!(terminals[0].id, terminal_id);
        assert_eq!(terminals[0].label, label);
        // A freshly-created terminal has no foreground command yet.
        assert_eq!(terminals[0].foreground_cmd, None);
    }

    #[test]
    fn spine_tags_a_project_terminal_with_its_project_owner() {
        let (mut engine, _tmp) = test_engine();

        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("create project terminal");

        let vm = engine.spine();
        // The one flat collection carries it, tagged with the PROJECT owner. It
        // is the tag, not which collection it arrived in, that says who owns it.
        assert_eq!(vm.terminals.len(), 1);
        assert_eq!(vm.terminals[0].id, terminal_id);
        assert_eq!(vm.terminals[0].label, label);
        assert_eq!(
            vm.terminals[0].owner,
            TerminalOwnerView::Project {
                project_id: "p1".to_string()
            },
            "a project terminal must never be tagged onto a session"
        );
    }

    /// The test that catches the silent-omission class of bug this shape exists
    /// to remove: EVERY terminal of EVERY owner kind reaches the client, in one
    /// collection, each carrying its own owner, in `sort_order` order.
    #[test]
    fn spine_terminals_carry_every_terminal_of_every_owner_kind() {
        let (mut engine, _worktree) = engine_with_spawnable_terminals();

        let (session_terminal, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_terminal, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");
        let (standalone_terminal, _) = engine
            .create_standalone_terminal(24, 80)
            .expect("standalone terminal");

        let vm = engine.spine();
        let ids: Vec<&str> = vm.terminals.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                session_terminal.as_str(),
                project_terminal.as_str(),
                standalone_terminal.as_str()
            ],
            "every owner kind appears, in spawn (sort_order) order"
        );
        let owners: Vec<&TerminalOwnerView> = vm.terminals.iter().map(|t| &t.owner).collect();
        assert_eq!(
            owners,
            vec![
                &TerminalOwnerView::Session {
                    session_id: "s1".to_string()
                },
                &TerminalOwnerView::Project {
                    project_id: "p1".to_string()
                },
                &TerminalOwnerView::Standalone {
                    cwd_label: crate::home_path::shorten_home(
                        &crate::home_path::standalone_terminal_dir()
                    )
                },
            ]
        );
    }

    /// The owner is a TAGGED union on the wire, so the client can switch on it.
    #[test]
    fn terminal_owner_serializes_with_a_kind_tag() {
        let session = serde_json::to_value(TerminalOwnerView::Session {
            session_id: "s1".to_string(),
        })
        .expect("serialize");
        assert_eq!(
            session,
            serde_json::json!({ "kind": "session", "session_id": "s1" })
        );
        let project = serde_json::to_value(TerminalOwnerView::Project {
            project_id: "p1".to_string(),
        })
        .expect("serialize");
        assert_eq!(
            project,
            serde_json::json!({ "kind": "project", "project_id": "p1" })
        );
        // The standalone tag carries no id (there is no owner) and instead
        // carries the thing its row names it by: the `~`-shortened directory.
        let standalone = serde_json::to_value(TerminalOwnerView::Standalone {
            cwd_label: "~/code".to_string(),
        })
        .expect("serialize");
        assert_eq!(
            standalone,
            serde_json::json!({ "kind": "standalone", "cwd_label": "~/code" })
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, _label) = engine
            .create_companion_terminal("s1", 24, 80)
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
            vm.terminals[0].foreground_cmd.as_deref(),
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
            vm.terminals[0].foreground_cmd, None,
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
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
        engine.providers.insert(TabId::new("s1-slot"), client);

        // Before any output, the session is not ready.
        assert!(!engine.spine().sessions[0].has_output);

        engine
            .providers
            .get(TabIdRef::new("s1-slot"))
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
    fn terminal_font_settings_are_projected() {
        let (mut engine, _tmp) = test_engine();

        // Defaults ship empty family / 14px.
        assert_eq!(engine.bootstrap().terminal_font_family, "");
        assert_eq!(engine.bootstrap().terminal_font_size, 14);

        engine.config.ui.terminal_font_family = "Fira Code".to_string();
        engine.config.ui.terminal_font_size = 18;
        let bootstrap = engine.bootstrap();
        assert_eq!(bootstrap.terminal_font_family, "Fira Code");
        assert_eq!(bootstrap.terminal_font_size, 18);
    }

    #[test]
    fn terminal_font_size_out_of_range_degrades_to_the_default_in_bootstrap() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.terminal_font_size = 200;
        assert_eq!(engine.bootstrap().terminal_font_size, 14);
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
    fn the_tailscale_mode_is_projected_as_its_canonical_name() {
        let (mut engine, _tmp) = test_engine();
        assert_eq!(engine.bootstrap().tailscale_mode, "auto");

        engine.config.server.tailscale = "  YES ".to_string();
        assert_eq!(
            engine.bootstrap().tailscale_mode,
            "yes",
            "the canonical name, not the raw text: a retyped value must not look \
             like a different mode to the row that renders it"
        );

        engine.config.server.tailscale = "maybe".to_string();
        assert_eq!(
            engine.bootstrap().tailscale_mode,
            "auto",
            "a value dux does not know projects as what the serve path degrades it to"
        );

        assert!(
            !engine.bootstrap().tailscale_forced_no,
            "the engine never knows about --no-tailscale; the serving process injects it"
        );
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
    fn tab_reaches_agent_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // Default ships off: Tab keeps moving between panes.
        assert!(!engine.bootstrap().tab_reaches_agent);

        engine.config.ui.tab_reaches_agent = true;
        assert!(engine.bootstrap().tab_reaches_agent);
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
    fn agent_sort_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        // Default is "active" so a fresh client sorts working agents to the top.
        assert_eq!(engine.bootstrap().agent_sort, "active");

        // A persisted mode projects verbatim so every client's sort control agrees
        // and a saved manual order stays visible across restarts.
        engine.config.ui.agent_sort = "manual".to_string();
        assert_eq!(engine.bootstrap().agent_sort, "manual");
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
    fn compose_bar_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        assert_eq!(engine.bootstrap().compose_bar, "auto");

        engine.config.ui.compose_bar = "never".to_string();
        assert_eq!(engine.bootstrap().compose_bar, "never");

        engine.config.ui.compose_bar = "always".to_string();
        assert_eq!(engine.bootstrap().compose_bar, "always");
    }

    #[test]
    fn compose_bar_projection_normalizes_an_unknown_mode() {
        let (mut engine, _tmp) = test_engine();

        engine.config.ui.compose_bar = "sometimes".to_string();
        assert_eq!(engine.bootstrap().compose_bar, "auto");
    }

    #[test]
    fn mobile_bar_preferences_are_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        assert!(engine.bootstrap().mobile_top_bar);
        assert!(engine.bootstrap().mobile_accessory_bar);

        engine.config.ui.mobile_top_bar = false;
        assert!(!engine.bootstrap().mobile_top_bar);
        assert!(engine.bootstrap().mobile_accessory_bar);

        engine.config.ui.mobile_accessory_bar = false;
        assert!(!engine.bootstrap().mobile_accessory_bar);
    }

    #[test]
    fn the_upload_gitignore_choice_is_projected_from_config() {
        // The web's Preferences dialog reads every row's current value off the
        // bootstrap document, so a row with no field here can only ever show
        // its documented default and would silently misreport a user who had
        // turned it off in `config.toml`.
        let (mut engine, _tmp) = test_engine();

        assert!(engine.bootstrap().upload_write_gitignore);

        engine.config.ui.upload_write_gitignore = false;
        assert!(!engine.bootstrap().upload_write_gitignore);
    }

    #[test]
    fn auto_reopen_agents_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        assert!(!engine.bootstrap().auto_reopen_agents);

        engine.config.ui.auto_reopen_agents = true;
        assert!(engine.bootstrap().auto_reopen_agents);
    }

    #[test]
    fn global_default_provider_is_projected_from_config() {
        let (mut engine, _tmp) = test_engine();

        assert_eq!(engine.bootstrap().global_default_provider, "claude");

        engine.config.defaults.provider = "codex".to_string();
        assert_eq!(engine.bootstrap().global_default_provider, "codex");
    }

    #[test]
    fn the_file_drop_size_cap_is_projected_so_the_browser_can_hide_a_disabled_feature() {
        // Zero is documented as switching file drop OFF, and the server refuses
        // every upload when it is. Without the value in the bootstrap document
        // the browser had no way to know: it still advertised a drop target,
        // still accepted the drop, and only then collected a refusal per file.
        // The server's refusal stays the enforcement; this is what lets a
        // disabled feature offer nothing.
        let (mut engine, _tmp) = test_engine();
        assert_eq!(
            engine.bootstrap().file_drop_max_bytes,
            crate::config::DEFAULT_FILE_DROP_MAX_BYTES
        );
        engine.config.server.file_drop_max_bytes = 0;
        assert_eq!(engine.bootstrap().file_drop_max_bytes, 0);
        engine.config.server.file_drop_max_bytes = 4242;
        assert_eq!(engine.bootstrap().file_drop_max_bytes, 4242);
    }

    #[test]
    fn the_long_paste_threshold_is_projected_already_normalized() {
        // The browser acts on this number without re-checking it, so an
        // out-of-range value must be corrected before it leaves. `set_settings`
        // and the raw config editor can both put one in memory without going
        // back through `load_config`, which is the only other place that
        // corrects it.
        let (mut engine, _tmp) = test_engine();
        assert_eq!(
            engine.bootstrap().upload_pasted_text_chars,
            crate::config::DEFAULT_UPLOAD_PASTED_TEXT_CHARS
        );
        // Zero is the off switch and survives untouched.
        engine.config.ui.upload_pasted_text_chars = 0;
        assert_eq!(engine.bootstrap().upload_pasted_text_chars, 0);
        // A value that would turn every prompt into a file is clamped up.
        engine.config.ui.upload_pasted_text_chars = 4;
        assert_eq!(
            engine.bootstrap().upload_pasted_text_chars,
            crate::config::MIN_UPLOAD_PASTED_TEXT_CHARS
        );
        engine.config.ui.upload_pasted_text_chars = 2_500;
        assert_eq!(engine.bootstrap().upload_pasted_text_chars, 2_500);
    }

    /// One launched entry, spelled out rather than defaulted, so a test that
    /// cares about the command says so and a test that cares about the form says
    /// so.
    fn launched(
        provider: &str,
        form: crate::config::WebDragDropPaste,
        command_name: &str,
    ) -> crate::engine::LaunchedDropPaste {
        crate::engine::LaunchedDropPaste {
            provider: provider.to_string(),
            form,
            command_name: command_name.to_string(),
        }
    }

    /// A session with an extra tab, both of which the spine will project.
    fn engine_with_two_tabs() -> (Engine, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat/x"));
        engine.agent_tabs.insert(
            TabId::new("tab-b"),
            crate::model::AgentTab {
                id: "tab-b".to_string(),
                session_id: "s1".to_string(),
                provider: ProviderKind::new("codex"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        (engine, tmp)
    }

    fn tab_drop_paste(engine: &Engine, tab_id: &str) -> Option<DropPasteView> {
        engine
            .session_view("s1")
            .expect("session projects")
            .tabs
            .into_iter()
            .find(|t| t.id == tab_id)
            .expect("tab projects")
            .drop_paste
    }

    #[test]
    fn the_spine_answers_per_tab_when_two_tabs_of_one_provider_launched_differently() {
        // THE CASE A PROVIDER-KEYED MAP CANNOT ANSWER. Launch a tab under
        // `codex`, edit `[providers.codex] web_dragdrop_paste`, launch a second
        // tab. Both processes are live, both report the provider name `codex`,
        // and they need DIFFERENT forms. A map keyed by provider name has one
        // slot for two answers, so whichever entry won gave one of the two panes
        // the wrong form, and which one won depended on `HashMap` iteration
        // order: nondeterministic, and wrong either way.
        //
        // So the launched profile is published per TAB, on the tab the browser
        // already has when a file lands on a pane.
        let (mut engine, _tmp) = engine_with_two_tabs();
        engine.launched_drop_paste.insert(
            TabId::new("s1-slot"),
            launched(
                "codex",
                crate::config::WebDragDropPaste::SingleQuoted,
                "codex",
            ),
        );
        engine.launched_drop_paste.insert(
            TabId::new("tab-b"),
            launched(
                "codex",
                crate::config::WebDragDropPaste::BackslashEscaped,
                "codex",
            ),
        );
        assert_eq!(
            tab_drop_paste(&engine, "s1-slot").map(|d| d.form),
            Some("single_quoted".to_string()),
            "each live tab keeps the form its own process launched with"
        );
        assert_eq!(
            tab_drop_paste(&engine, "tab-b").map(|d| d.form),
            Some("backslash_escaped".to_string()),
            "...including a sibling tab of the same provider that launched later"
        );
    }

    /// The failure verdict reaches the browser per tab, uniformly: the data
    /// carries no notion of which tab is in the session slot, because the rule
    /// that reads it is the surface's, not the engine's.
    #[test]
    fn a_tabs_failed_run_is_published_per_tab() {
        let (mut engine, _tmp) = engine_with_two_tabs();
        let failed = |engine: &Engine, tab_id: &str| {
            engine
                .session_view("s1")
                .expect("session projects")
                .tabs
                .into_iter()
                .find(|t| t.id == tab_id)
                .expect("tab projects")
                .last_run_failed
        };
        assert!(!failed(&engine, "s1-slot"));
        assert!(!failed(&engine, "tab-b"));

        engine.mark_tab_run_failed(TabIdRef::new("s1-slot"));
        assert!(failed(&engine, "s1-slot"));
        assert!(
            !failed(&engine, "tab-b"),
            "one tab's bad run says nothing about its siblings"
        );

        engine.clear_tab_run_failure(TabIdRef::new("s1-slot"));
        assert!(!failed(&engine, "s1-slot"));
    }

    #[test]
    fn a_tabs_launched_profile_appears_and_retires_with_its_process() {
        // WHY IT RIDES THE SPINE. The entry appears when a process launches and
        // goes when it is torn down, and the spine is what a launch and a
        // termination refresh. On the bootstrap document (refreshed only by
        // `config.changed`) the browser's copy went stale for the whole life of
        // a process: a client that had refetched config before a relaunch kept
        // resolving the OLD entry.
        let (mut engine, _tmp) = engine_with_two_tabs();
        assert_eq!(
            tab_drop_paste(&engine, "s1-slot"),
            None,
            "a tab with no live process has no launched profile"
        );
        engine.launched_drop_paste.insert(
            TabId::new("s1-slot"),
            launched(
                "codex",
                crate::config::WebDragDropPaste::SingleQuoted,
                "codex",
            ),
        );
        assert_eq!(
            tab_drop_paste(&engine, "s1-slot").map(|d| d.form),
            Some("single_quoted".to_string()),
            "a launch publishes it on the same spine the launch refreshes"
        );
        engine.clear_tab_runtime(TabIdRef::new("s1-slot"));
        assert_eq!(
            tab_drop_paste(&engine, "s1-slot"),
            None,
            "and a termination retires it on that same spine"
        );
    }

    #[test]
    fn a_live_tab_keeps_its_profile_after_its_provider_block_is_renamed() {
        // A user renames or deletes a `[providers.<name>]` block while a tab is
        // still running that provider. The tab's own `provider` string does not
        // change (it is what actually launched), so a browser looking that name
        // up in the config-derived map finds nothing and falls back to the
        // default: a live codex tab would start receiving unquoted paths
        // mid-session, which is precisely the case codex silently ignores.
        //
        // The launched profile is therefore kept with the PROCESS. The
        // alternative was to refuse the rename, and a config file the user edits
        // in their own editor cannot be refused.
        let (mut engine, _tmp) = engine_with_two_tabs();
        engine.launched_drop_paste.insert(
            TabId::new("s1-slot"),
            launched(
                "codex-nightly",
                crate::config::WebDragDropPaste::SingleQuoted,
                "codex",
            ),
        );
        assert_eq!(
            tab_drop_paste(&engine, "s1-slot").map(|d| d.form),
            Some("single_quoted".to_string()),
            "a live process keeps what it launched with, even once config no \
             longer names its provider"
        );
        // And it is NOT smuggled into the config-derived map, which stays a
        // plain projection of config. Two different questions, two different
        // places.
        assert!(
            !engine
                .bootstrap()
                .provider_drop_paste
                .contains_key("codex-nightly"),
            "the provider map answers for CONFIG, never for a live process"
        );
    }

    #[test]
    fn bootstrap_provider_map_is_purely_config_derived() {
        // The provider map is the fallback for a pane with no live process, so
        // it must always report what config says right now rather than what
        // some process started with.
        let (mut engine, _tmp) = test_engine();
        engine.launched_drop_paste.insert(
            TabId::new("s2"),
            launched(
                "claude",
                crate::config::WebDragDropPaste::BackslashEscaped,
                "claude",
            ),
        );
        let forms = engine.bootstrap().provider_drop_paste;
        assert_eq!(
            forms.get("claude").map(|d| d.form.as_str()),
            Some("bare"),
            "config is what the provider map reports"
        );
    }

    #[test]
    fn a_provider_is_published_by_the_command_it_runs_not_by_its_block_name() {
        // The browser's measured per-CLI paste-length table is keyed by the CLI,
        // and the only thing in a provider block that names the CLI is its
        // `command`. Keyed by the BLOCK NAME (which it was) it answered for the
        // wrong tool in both directions: a real Codex under an alias got no
        // limit and was handed oversized paths it silently ignores, and an
        // unrelated CLI merely NAMED codex had valid long paths withheld.
        let (mut engine, _tmp) = test_engine();
        engine.config.providers.commands.insert(
            "myagent".to_string(),
            crate::config::ProviderCommandConfig {
                command: "/usr/local/bin/codex".to_string(),
                args: Vec::new(),
                resume_args: None,
                resume_wait_timeout_ms: None,
                install_hint: None,
                forward_scroll: None,
                web_dragdrop_paste: None,
            },
        );
        engine.config.providers.commands.insert(
            "codex".to_string(),
            crate::config::ProviderCommandConfig {
                command: "something-else".to_string(),
                args: Vec::new(),
                resume_args: None,
                resume_wait_timeout_ms: None,
                install_hint: None,
                forward_scroll: None,
                web_dragdrop_paste: None,
            },
        );
        let forms = engine.bootstrap().provider_drop_paste;
        assert_eq!(
            forms.get("myagent").map(|d| d.command_name.as_str()),
            Some("codex"),
            "an aliased block running codex is published as codex, by file name"
        );
        assert_eq!(
            forms.get("codex").map(|d| d.command_name.as_str()),
            Some("something-else"),
            "a block merely NAMED codex is published as whatever it runs"
        );
    }

    /// `capabilities.passthrough` governs what an agent forwards OUTWARD, and on
    /// the web the only such thing is the OSC 52 clipboard write. It is resolved
    /// here, into the published clipboard mode, so the browser reads one answer
    /// rather than combining two. It deliberately does NOT reach browser desktop
    /// notifications: `web_notifications` is the only switch for those.
    #[test]
    fn bootstrap_master_passthrough_off_seals_the_clipboard_only() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = false;
        engine.config.capabilities.clipboard_passthrough = "always".to_string();
        engine.config.capabilities.web_notifications = true;

        let b = engine.bootstrap();
        assert_eq!(
            b.clipboard_passthrough, "off",
            "master off must normalize the clipboard mode to off"
        );
        assert!(
            b.web_notifications,
            "passthrough must not silence browser notifications; web_notifications owns those"
        );
    }

    /// The operator-facing case for the other switch: turning notifications off
    /// suppresses them whatever `passthrough` says.
    #[test]
    fn bootstrap_web_notifications_off_is_independent_of_passthrough() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = true;
        engine.config.capabilities.web_notifications = false;

        let b = engine.bootstrap();
        assert!(!b.web_notifications);
    }

    #[test]
    fn bootstrap_master_passthrough_on_keeps_the_configured_clipboard_mode() {
        let (mut engine, _tmp) = test_engine();
        engine.config.capabilities.passthrough = true;
        engine.config.capabilities.clipboard_passthrough = "always".to_string();

        let b = engine.bootstrap();
        assert_eq!(b.clipboard_passthrough, "always");
    }

    /// The resolved clipboard mode is the ONLY thing the master switch publishes
    /// to the browser. A second raw `passthrough` field would be a value the
    /// client had to combine, and the only web consumer already has its answer.
    #[test]
    fn bootstrap_does_not_publish_the_raw_master_switch() {
        let (engine, _tmp) = test_engine();
        let v = serde_json::to_value(engine.bootstrap()).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(
            !obj.contains_key("passthrough"),
            "the master switch is resolved into clipboard_passthrough, not published raw"
        );
    }

    #[test]
    fn bootstrap_serializes_to_json_with_expected_fields() {
        let (engine, _tmp) = test_engine();
        let json = serde_json::to_string(&engine.bootstrap()).expect("serialize");
        for field in [
            "available_providers",
            "provider_drop_paste",
            "macros",
            "welcome_tips",
            "dux_version",
            "randomize_agent_names_by_default",
            "gh_available",
            "github_integration",
            "copy_on_select",
            "terminal_font_family",
            "terminal_font_size",
            "compose_bar",
            "mobile_top_bar",
            "mobile_accessory_bar",
            "upload_write_gitignore",
            "upload_pasted_text_chars",
            "auto_reopen_agents",
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
            "tab_reaches_agent",
            "attention_indicator",
            "attention_on_bell",
            "global_default_provider",
            "welcome_screen",
            "website_url",
            "pending_first_load",
            "disable_automated_welcome_screen",
            "disable_release_notes",
            "file_drop_max_bytes",
            "tailscale_mode",
            "tailscale_forced_no",
            "replay_wait_seconds",
            "reconnect_backoff_cap_seconds",
            "heartbeat_seconds",
            "heartbeat_deadline_seconds",
        ] {
            assert!(
                json.contains(&format!("\"{field}\"")),
                "bootstrap JSON must carry {field}: {json}"
            );
        }
    }

    /// The four reconnect timings are projected verbatim from `[server]`.
    ///
    /// Nothing in the server reads them: they exist for the BROWSER's attach
    /// state machine, which can only learn them through this document. A config
    /// reload refetches the bootstrap, so editing the file retimes every open
    /// tab with no restart.
    #[test]
    fn bootstrap_projects_the_reconnect_timings_from_server_config() {
        let (mut engine, _tmp) = test_engine();
        let b = engine.bootstrap();
        assert_eq!(
            b.replay_wait_seconds,
            crate::config::DEFAULT_REPLAY_WAIT_SECONDS
        );
        assert_eq!(
            b.reconnect_backoff_cap_seconds,
            crate::config::DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS
        );
        assert_eq!(
            b.heartbeat_seconds,
            crate::config::DEFAULT_HEARTBEAT_SECONDS
        );
        assert_eq!(
            b.heartbeat_deadline_seconds,
            crate::config::DEFAULT_HEARTBEAT_DEADLINE_SECONDS
        );

        engine.config.server.replay_wait_seconds = 3;
        engine.config.server.reconnect_backoff_cap_seconds = 4;
        engine.config.server.heartbeat_seconds = 5;
        engine.config.server.heartbeat_deadline_seconds = 6;
        let b = engine.bootstrap();
        assert_eq!(b.replay_wait_seconds, 3);
        assert_eq!(b.reconnect_backoff_cap_seconds, 4);
        assert_eq!(b.heartbeat_seconds, 5);
        assert_eq!(b.heartbeat_deadline_seconds, 6);
    }

    #[test]
    fn the_welcome_screen_content_is_projected_from_core_and_names_the_config_path() {
        // The web renders the SAME prose the TUI does; core owns the copy, and
        // the last paragraph interpolates this machine's config path. Projected
        // unconditionally (not only when the welcome is pending) because the app
        // menu can open the screen on demand at any time.
        let (engine, _tmp) = test_engine();
        let b = engine.bootstrap();
        assert_eq!(b.welcome_screen.tagline, crate::welcome_screen::TAGLINE);
        assert_eq!(b.welcome_screen.steps.len(), 3);
        assert_eq!(b.welcome_screen.steps[0].number, 1);
        assert_eq!(b.welcome_screen.steps[0].title, "Add a project");
        let joined = b.welcome_screen.paragraphs.join(" ");
        assert!(
            joined.contains(&engine.paths.config_path.display().to_string()),
            "the resolved config path must be projected verbatim: {joined}"
        );
    }

    #[test]
    fn the_website_url_is_projected_from_the_one_url_home() {
        // The welcome screen's secondary button links here. Projected rather
        // than hardcoded in the client so the two surfaces cannot disagree.
        let (engine, _tmp) = test_engine();
        assert_eq!(engine.bootstrap().website_url, crate::urls::WEBSITE);
    }

    #[test]
    fn no_screen_is_pending_in_the_bare_projection() {
        // The pending first-load screen is NOT engine state: the web server
        // computes the plan once at startup, holds it in memory, and injects it
        // into this document per request. The projection always says `None` so
        // the field's absence is never mistaken for "the engine decided no".
        let (engine, _tmp) = test_engine();
        assert!(engine.bootstrap().pending_first_load.is_none());
    }

    #[test]
    fn the_two_first_load_disable_flags_are_projected_from_config() {
        let (mut engine, _tmp) = test_engine();
        let b = engine.bootstrap();
        assert!(!b.disable_automated_welcome_screen);
        assert!(!b.disable_release_notes);

        engine.config.ui.disable_automated_welcome_screen = true;
        engine.config.ui.disable_release_notes = true;
        let b = engine.bootstrap();
        assert!(b.disable_automated_welcome_screen);
        assert!(b.disable_release_notes);
    }

    #[test]
    fn a_pending_screen_serializes_with_its_screen_name_and_notes() {
        // The wire shape the web client branches on: a screen name plus the
        // notes, present only for the what's-new screen.
        let welcome = PendingFirstLoadView::welcome();
        let json = serde_json::to_string(&welcome).expect("serialize");
        assert!(json.contains("\"screen\":\"welcome\""), "{json}");
        assert!(json.contains("\"notes\":null"), "{json}");

        let notes = crate::release_notes::ReleaseNotes {
            version: "v0.7.0".to_string(),
            headline: "Louder failures".to_string(),
            ..Default::default()
        };
        let whats_new = PendingFirstLoadView::whats_new(notes);
        let json = serde_json::to_string(&whats_new).expect("serialize");
        assert!(json.contains("\"screen\":\"whats_new\""), "{json}");
        assert!(json.contains("\"version\":\"v0.7.0\""), "{json}");
        assert!(json.contains("Louder failures"), "{json}");
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
