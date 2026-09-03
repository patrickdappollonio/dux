//! Transport-agnostic command intake. A web client sends `{command, args}` JSON;
//! it deserializes into `WireCommand`, is reconstructed into the engine's
//! `Command` (looking up domain objects by id server-side), and dispatched
//! through the same `Engine::apply` the TUI uses. The result is downsampled to a
//! wire-safe `WireCommandOutcome` (the full `EventReaction` is engine-internal
//! and view-coupled; web clients re-fetch `view_model()` for fresh state).

/// Stable string keys for correlated status pairs (busy → final). The TUI and
/// the web actor MUST use the SAME string so a final status replaces the busy
/// on the same key slot rather than opening a second slot.
///
/// Keys are per-OPERATION-TYPE where the operation is globally unique (e.g.
/// config reload), and per-OPERATION-INSTANCE (including the entity id) where
/// multiple operations of the same type can be in flight concurrently (e.g.
/// per-session launch or delete).
pub mod status_keys {
    /// Config-reload failure key. Singleton: only one reload can run at a time.
    pub const CONFIG_RELOAD: &str = "config-reload";
    /// Worktree-list failure key. Parameterised by project id at call sites:
    /// `format!("{WORKTREE_LIST_PREFIX}:{project_id}")`.
    pub const WORKTREE_LIST_PREFIX: &str = "worktree-list";
    // The checkout-default, add-project-checkout, pr-lookup, async worktree-delete,
    // create-agent, and reconnect/force-restart launch operations no longer use
    // hand-authored key prefixes: their busies carry the opaque id of a
    // `HandlerStatusOp` (see `Engine::pending_web_*_ops`,
    // `Engine::pending_delete_ops_web`, `Engine::pending_create_ops`, and
    // `Engine::pending_web_launch_ops`) so the busy and its final correlate without
    // a shared string. The clear-workarounds they needed
    // (`web_completed_busy_key_to_clear`, `web_launch_ready_keys_to_clear`) were
    // removed with them.
    /// Push key prefix. Parameterised by worktree path at call sites:
    /// `format!("{PUSH_PREFIX}:{worktree_path}")`.
    pub const PUSH_PREFIX: &str = "push";

    /// Push operation key, parameterised by worktree path.
    pub fn push(worktree_path: &str) -> String {
        format!("{PUSH_PREFIX}:{worktree_path}")
    }
}

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::{
    AgentLaunchFailedOutcome, AgentLaunchReadyView, BeginDeleteSessionOutcome, CloseTabOutcome,
    Command, Engine, EventReaction, FinishDeleteSessionOutcome, ProjectPersistenceView,
    StatusUpdate, WorktreeRemoval,
};
use crate::ids::{SessionIdRef, TabId, TabIdRef};
use crate::model::{Project, ProjectBranchStatus, ProviderKind};
use crate::statusline::StatusScope;
use crate::worker::{
    CreateAgentRequest, NonDefaultBranchAction, ProjectPersistenceAction, PullTarget,
};

/// A command as received from a generic transport (e.g. the web WebSocket).
/// `#[serde(tag = "command", content = "args")]` matches the `{ "command": "...",
/// "args": { ... } }` envelope.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum WireCommand {
    StageFile {
        session_id: String,
        path: String,
    },
    UnstageFile {
        session_id: String,
        path: String,
    },
    /// Discard a single file's working-tree changes. The wire layer derives the
    /// destructive distinction (tracked → restore from HEAD vs untracked →
    /// delete the file) SERVER-SIDE from the worktree's live git status, and
    /// rejects the command when the file is currently staged — mirroring the
    /// TUI, which only allows discarding unstaged files.
    DiscardFile {
        session_id: String,
        path: String,
    },
    CommitChanges {
        session_id: String,
        message: String,
    },
    Push {
        session_id: String,
    },
    Pull {
        session_id: String,
    },
    /// Pull (refresh) a PROJECT's source checkout from remote, mirroring the
    /// TUI's `refresh_selected_project`. Unlike [`WireCommand::Pull`] (which
    /// targets a session's worktree), this resolves the project by id and runs
    /// `Command::Pull` against the project's source checkout path with
    /// `PullTarget::Project`. The busy/already-running copy and the leading
    /// branch source match the TUI byte-for-byte.
    PullProject {
        project_id: String,
    },
    ToggleAgentAutoReopen {
        session_id: String,
        enabled: bool,
    },
    /// Re-run the agent's project startup command in that agent's worktree,
    /// mirroring the TUI's `rerun-startup-command-on-agent` palette command. The
    /// wire layer resolves the session and its project server-side, merges the
    /// global + project env, and runs the command off-thread (a keyed Busy →
    /// final status pair flows back through `spawn_status_op`). Rejected when the
    /// session/project is unknown or the project has no startup command.
    RerunStartupCommand {
        session_id: String,
    },
    DeleteTerminal {
        terminal_id: String,
    },
    DeleteSession {
        session_id: String,
        delete_worktree: bool,
        /// The delete dialog's "also delete the branch" answer. ABSENT means
        /// nobody was asked, which keeps the provenance default and is what a
        /// caller that only knows about `delete_worktree` gets. See
        /// [`crate::model::BranchProvenance::resolve_branch_deletion`].
        #[serde(default)]
        delete_branch: Option<bool>,
    },
    PersistGlobalEnv {
        env: BTreeMap<String, String>,
    },
    UpdateProjectProvider {
        project_id: String,
        provider: Option<String>,
    },
    UpdateProjectAutoReopen {
        project_id: String,
        auto_reopen_agents: Option<bool>,
    },
    UpdateProjectStartupCommand {
        project_id: String,
        startup_command: Option<String>,
    },
    UpdateProjectEnv {
        project_id: String,
        env: BTreeMap<String, String>,
    },
    /// Re-read `config.toml` from disk and apply it to the running engine.
    ///
    /// Modeled as an empty struct variant (not a unit variant) so it deserializes
    /// from both `{"command":"reload_config"}` and `{"command":"reload_config",
    /// "args":{}}`. The frontend's generic command envelope always carries an
    /// `args` object, and serde's `content="args"` tagging rejects a map for a
    /// true unit variant — an empty struct variant accepts both forms.
    ReloadConfig {},
    /// Overwrite `config.toml` from the current in-memory config. Empty struct
    /// variant for the same reason as [`WireCommand::ReloadConfig`].
    RecoverConfig {},
    /// Persist the Changes (git) pane's visibility to `config.toml`
    /// (`ui.show_changes_pane`). Sent by the web's hide/show toggle so the
    /// choice sticks across restarts; the next ViewModel broadcast carries the
    /// new value, and the client reconciles its optimistic override against it.
    SetChangesPaneVisible {
        visible: bool,
    },
    /// Persist this dux instance's identity to `config.toml`: the browser tab
    /// `<title>` (`config.server.title`) and the favicon color
    /// (`config.server.favicon`). Sent by the web's customize-webapp dialog. Each
    /// field is optional so a single-field body (just a title, or just a color)
    /// only touches that field; an empty body (both `None`) is a no-op (no write,
    /// no `config.changed`). The title is normalized (control + bidi/format chars
    /// neutralized, whitespace collapsed, capped, empty resets to "dux"); the
    /// favicon must be a curated color name (or empty to reset) or the command is
    /// rejected. The write is eager so the endpoint can report a synchronous
    /// success/failure, and a non-empty body mutates config-static state so the web
    /// fires `config.changed` and every tab refetches its title + favicon.
    SetInstanceIdentity {
        title: Option<String>,
        favicon: Option<String>,
    },
    /// Flip `defaults.enable_randomized_pet_name_by_default` and persist it,
    /// mirroring the TUI's `toggle-randomized-pet-name-default` palette command.
    /// The server is the source of truth, so this is a parameterless toggle: the
    /// engine reads the current value and flips it (two rapid clicks cancel out,
    /// matching the TUI). A low-stakes preference, so the write is lazy.
    ToggleRandomizedPetNameDefault {},
    /// Flip `ui.pr_banner_position` between "top" and "bottom" and persist it,
    /// mirroring the TUI's `toggle-pr-banner-position` palette command. The next
    /// `config.changed` refetch carries the new value so the web moves the PR
    /// banner lane. Low-stakes preference, lazy write.
    TogglePrBannerPosition {},
    /// Set `ui.agent_sort` to an explicit mode ("active" | "updated" | "created"
    /// | "name" | "name_desc" | "manual") and persist it. Unlike the toggles, this carries a
    /// value: the web sidebar's sort control sets it directly, and a drag-reorder
    /// sets it to "manual". The next `config.changed` refetch carries the new value
    /// so every client's sort control agrees. Unknown values are rejected. Low-stakes
    /// preference, lazy write.
    SetAgentSort {
        sort: String,
    },
    /// Flip `ui.github_integration` and persist it, mirroring the TUI's
    /// `toggle-github-integration` palette command. Beyond the config flag this
    /// drives the engine's PR-sync side effects (start/stop background PR refresh,
    /// clear cached PR statuses) so the running server actually starts or stops
    /// talking to `gh`. The write is eager because the user wants to know if
    /// persisting the toggle failed.
    ToggleGithubIntegration {},
    /// Ask `gh` again right now, mirroring the TUI's `recheck-github` palette
    /// command. Writes no config: it is the way out of a `gh` that failed for a
    /// reason that has since passed, for a user who cannot restart dux without
    /// taking every running agent down with it.
    RecheckGithub {},
    /// Manually attach (pin) an already-RESOLVED pull request to a session. The
    /// fields carry what `gh pr view` answered; parsing/resolution of the raw
    /// reference happens earlier (the attach lookup worker), so applying this is
    /// a synchronous row + map mutation and mints no busy of its own; the
    /// resolve→attach flow's single keyed op carries the busy and the final.
    AttachPullRequest {
        session_id: String,
        host: String,
        owner_repo: String,
        number: u64,
        title: String,
        state: String,
        url: String,
    },
    /// Detach a session's pull request: this agent has no PR, as of now. The
    /// pin goes if there was one, the badge clears immediately, and
    /// autodetection is suppressed for the session (durably) so it cannot put
    /// the badge straight back. Synchronous. Applies to an autodetected
    /// association too, not only to a pin. Reversed by an
    /// `AttachPullRequest` or a `ResumePullRequestAutodetection`.
    ClearPullRequestOverride {
        session_id: String,
    },
    /// Undo a detach: switch pull-request autodetection back on for the
    /// session and run one immediate check, so the badge returns now rather
    /// than at the next poll. Synchronous, and deliberately not gated on `gh`
    /// (the suppression is dux's own state); without a usable `gh` the check
    /// is a no-op and the session is picked up once the integration re-arms.
    ResumePullRequestAutodetection {
        session_id: String,
    },
    /// Flip `ui.copy_on_select` and persist it, mirroring the web
    /// `toggle-copy-on-select` palette command. The next `config.changed` refetch
    /// carries the new value so the web terminal enables or disables
    /// highlight-to-copy. Low-stakes preference, lazy write.
    ToggleCopyOnSelect {},
    /// Flip `ui.always_show_tab_strip` and persist it, mirroring the TUI's
    /// `toggle-always-show-tabs` palette command. The next `config.changed`
    /// refetch carries the new value so the web shows (or stops showing) the
    /// agent tab strip at a single tab. Low-stakes preference, lazy write.
    ToggleAlwaysShowTabStrip {},
    /// Flip `ui.tab_reaches_agent` and persist it, mirroring the TUI's
    /// `toggle-tab-to-agent` palette command. Only the TUI acts on the value; the
    /// web carries the toggle so the setting can be changed from a browser.
    /// Low-stakes preference, lazy write.
    ToggleTabReachesAgent {},
    /// Save `[server] tailscale` as one of the three modes, mirroring the TUI's
    /// `set-tailscale-mode` palette command and the web Preferences row.
    ///
    /// The CONFIG write only. Moving the running listener is the serve layer's
    /// job and happens after this, because only the serve loop owns the leg; the
    /// engine has no idea whether anything is serving. An unrecognized value is
    /// refused rather than degraded, because this arrives from a picker and a
    /// value outside the tri-state is a bug, not a typo in a file.
    SetTailscaleMode {
        mode: String,
    },
    /// Set explicit values for the "Settings" modal's `[ui]`/`[capabilities]`
    /// knobs in one request, mirroring `SetInstanceIdentity`'s all-optional
    /// present-field pattern but for the generic settings surface (the
    /// Preferences dialog, opened from the app menu's cog, see
    /// `crates/dux-web/web/src/lib/settingsDescriptors.ts` for the exact
    /// field set and descriptions). Every field is optional; an
    /// absent field is left untouched, and a body with every field absent is a
    /// no-op (no write, no `config.changed`). Numeric fields are clamped to a
    /// sane documented ceiling (0 stays 0 where the config documents a zero
    /// meaning); `pr_banner_position` is validated against a fixed set of
    /// accepted values and the whole command is REJECTED (no partial apply)
    /// if it is present and unrecognized. `server.title` and
    /// `server.favicon` are deliberately NOT here, they stay on
    /// `SetInstanceIdentity`. The write is eager so the endpoint can report a
    /// synchronous success/failure, and a non-empty patch mutates config-static
    /// state so the web fires `config.changed` and every tab refetches its
    /// settings.
    /// The field list lives on [`SettingsPatch`], which carries the
    /// field-by-field semantics. The wire format is unaffected by the newtype:
    /// under this enum's adjacent tagging a newtype variant flattens its inner
    /// struct straight into `args`, so `{"command":"set_settings","args":{...}}`
    /// serializes and parses byte-identically to the inline-field form this
    /// replaced (pinned by `set_settings_wire_json_round_trips_through_args`).
    SetSettings(SettingsPatch),
    /// Force-kill a running agent's PTY WITHOUT deleting its session or
    /// worktree, the web counterpart to the TUI's kill-running modal (for one
    /// agent). Mirrors the force-reconnect teardown block but stops there (no
    /// relaunch): the provider is dropped (SIGKILL on Drop), resume state is
    /// cleared, and the session is marked Detached so it can be reconnected
    /// later. Companion terminals are killed through the existing
    /// `DeleteTerminal`. Unknown session is an `Err`; killing an agent that is
    /// not running is an idempotent no-op.
    KillSessionPty {
        session_id: String,
    },
    /// Detach an agent WHOLE: stop every one of its tabs' provider processes,
    /// mark the session Detached, and clear `desired_running` so startup
    /// auto-reopen does not relaunch it. This is the agent-level "Detach agent"
    /// action (the sidebar/menu), DISTINCT from closing a single tab (which stops
    /// only that tab and detaches the agent only when it was the last live one).
    /// The row and worktree are kept, so the agent can be reconnected. Unknown
    /// session is an `Err`; an agent with no live tabs is an idempotent no-op.
    DetachAgent {
        session_id: String,
    },
    /// Register an existing git repository on the server as a project. `name`
    /// may be empty to derive the display name from the path's basename.
    AddProject {
        path: String,
        name: String,
    },
    /// Check the repo's default branch out FIRST, then register it as a project,
    /// mirroring the TUI's "Check Out & Add" button in the
    /// `ConfirmNonDefaultBranch` dialog (the default action for the confident
    /// "Known" warning). Only valid when the repo's `origin/HEAD` resolves to a
    /// known default that differs from the current branch — the wire layer
    /// re-runs `branch_warning_kind` server-side and rejects the command
    /// otherwise (defense against a stale/forged path; the heuristic path never
    /// offers this option, matching the TUI).
    ///
    /// `git switch` rewrites the working tree, so this follows the L4 worker
    /// chain rather than running inline (CLAUDE.md workers tenet): it spawns
    /// `run_add_project_checkout_job` and returns a busy status. Worker
    /// completion posts `NonDefaultBranchCheckoutCompleted`, whose
    /// `AddProjectAfterBranchCheckout` reaction is driven to the actual project
    /// add by the web actor's `drive_add_project_followup` (the TUI drives the
    /// identical reaction from its `workers.rs` drain).
    AddProjectCheckoutDefault {
        path: String,
        name: String,
    },
    /// Create an empty initial commit for a fresh (unborn) repo, THEN register
    /// it as a project. Used when the client's inspect reported `has_commits:
    /// false`. Mirrors the L4 worker chain of `AddProjectCheckoutDefault`:
    /// validates the path, serializes per repo path via
    /// `InFlightKey::InitialCommit`, spawns `run_create_initial_commit_job`, and
    /// returns a busy status. Worker completion posts `InitialCommitCreated`,
    /// whose `AddProjectAfterInitialCommit` reaction is driven to the actual add
    /// by `drive_add_project_followup` (web) or the TUI's `workers.rs` drain.
    AddProjectCreateInitialCommit {
        path: String,
        name: String,
    },
    /// Initialize a brand-new git repository in a plain folder, seed a starter
    /// `.gitignore`, create an empty initial commit, THEN register it as a
    /// project (the adopt-a-folder flow). Mirrors the worker chain of
    /// `AddProjectCreateInitialCommit`: validates via
    /// `validate_project_init_path`, serializes per path via the shared
    /// `InFlightKey::InitialCommit`, spawns `run_init_repo_job`, and returns a
    /// busy status resolved by `drive_add_project_followup`.
    AddProjectInitRepo {
        path: String,
        name: String,
    },
    /// Remove a project from the workspace by id (does not touch its checkout).
    RemoveProject {
        project_id: String,
    },
    /// Delete a project by id AND remove its agents' worktrees from disk (the
    /// destructive counterpart to `RemoveProject`). Guarded and sequenced by
    /// `Command::DeleteProject`.
    DeleteProject {
        project_id: String,
    },
    /// Create a new agent in a project. `name` may be empty to auto-generate a
    /// branch name; a non-empty name becomes the custom branch/agent name.
    CreateAgent {
        project_id: String,
        name: String,
        /// Per-agent override for copying the project checkout's uncommitted
        /// changes into the new worktree. `None` falls back to the config
        /// default (`defaults.copy_uncommitted_changes_by_default`).
        #[serde(default)]
        copy_uncommitted_changes: Option<bool>,
        /// Whether the client has CONFIRMED attaching to an existing branch of
        /// the same name. `false`/absent means "not confirmed": the create runs
        /// the branch preflight and, if the name matches an existing branch,
        /// REFUSES with an existing-branch signal so the client can confirm
        /// (closing the web silent-attach bug). `true` means the client
        /// confirmed, so the create proceeds and adopts that branch's history.
        #[serde(default)]
        use_existing_branch: bool,
    },
    /// Create a STANDALONE agent: point a provider at a folder the user already
    /// has and run it there. No project, no branch, no worktree.
    ///
    /// Deliberately its own command rather than a `project_id: Option<String>`
    /// on [`WireCommand::CreateAgent`]: every field of that one is about a
    /// project's branch machinery, and an optional project id would make each
    /// of them separately meaningless.
    CreateStandaloneAgent {
        /// The folder to run in, as an absolute path on the SERVER's
        /// filesystem. Accepted whatever it contains; it does not have to be a
        /// repository.
        folder: String,
        /// The agent's display name. May be empty, in which case a sanitized
        /// form of the folder's own name is used. Folder names legally contain
        /// characters a branch name cannot, so the branch validator explicitly
        /// does NOT apply here.
        #[serde(default)]
        name: String,
        /// Which provider to launch. `None` uses the global default, the same
        /// source the config's own default uses. Retargetable afterward like
        /// any agent.
        #[serde(default)]
        provider: Option<String>,
    },
    /// Fork an existing agent session into a fresh worktree branched from the
    /// source session's branch. `name` follows the same rules as `CreateAgent`:
    /// empty auto-generates a branch name (server pet-name path), a non-empty
    /// name becomes the custom branch/agent name. Mirrors the TUI's
    /// `fork_selected_session`, which builds a `CreateAgentRequest::ForkSession`.
    ForkSession {
        session_id: String,
        name: String,
    },
    /// Rename an agent session's display title. `title` is trimmed; an empty
    /// title clears the custom name back to `None`, so the row reverts to
    /// showing the branch name. A non-empty title is validated with the same
    /// agent-name rules the TUI rename enforces. This is title-only: unlike the
    /// TUI prompt (which can also rename the git branch via a checkbox), the web
    /// rename never touches the branch — see `rename_session` for the rationale.
    RenameSession {
        session_id: String,
        title: String,
    },
    /// Reconnect (relaunch) an agent session's provider. `force == false`
    /// resumes the prior conversation when the provider supports it (the
    /// TUI's `reconnect_selected_session`); `force == true` always starts a
    /// fresh session with no resume args and first tears down any running
    /// provider (the TUI's `force_reconnect_agent`).
    ReconnectSession {
        session_id: String,
        force: bool,
    },
    /// Persist a custom display order for a project's agent sessions.
    /// `session_ids` must list exactly that project's sessions (the engine
    /// validates this strictly and errors otherwise). Drives the web UI's
    /// drag-and-drop reordering.
    ReorderSessions {
        project_id: String,
        session_ids: Vec<String>,
    },
    /// Persist a GLOBAL custom order for every agent (the flat model). `session_ids`
    /// must list exactly the full set of all sessions, in the desired order (the
    /// engine validates strictly). This is the web flat list's drag: agents reorder
    /// as one independent list, so a drop can land anywhere regardless of project.
    ReorderAgents {
        session_ids: Vec<String>,
    },
    /// Persist a custom display order for the workspace's projects.
    /// `project_ids` must list exactly the known projects.
    ReorderProjects {
        project_ids: Vec<String>,
    },
    /// Persist a GLOBAL runtime order for every companion terminal (session- and
    /// project-owned alike; the web renders one flat Terminals section).
    /// `terminal_ids` must list exactly the current set of terminals, in the
    /// desired order (the engine validates strictly). Runtime-only: terminals
    /// have no SQLite row, so this order resets to creation order on restart.
    ReorderTerminals {
        terminal_ids: Vec<String>,
    },
    /// Swap which CLI a session uses, mirroring the TUI palette's
    /// `change-agent-provider`. `provider` is validated server-side against the
    /// engine's configured provider list (the same source as the ViewModel's
    /// `available_providers`) — the client's choice is never trusted.
    ///
    /// Like the TUI, this does NOT kill or relaunch a running agent: it persists
    /// the new provider for the NEXT launch and, when a provider is still
    /// running on the session's PTY, pins the previously-running one so labels
    /// stay truthful until the user reconnects. Selecting the current provider
    /// is a no-op.
    ChangeAgentProvider {
        session_id: String,
        provider: String,
    },
    /// Close an extra tab (a non-Main provider tab): tear down its PTY and delete
    /// its `agent_tabs` row. Destructive and unrecoverable (extra tabs are
    /// ephemeral). The session-slot tab is never closed through here — a Main "close" is a
    /// detach via `KillSessionPty`. `tab_id` must belong to `session_id`.
    CloseAgentTab {
        session_id: String,
        tab_id: String,
    },
    /// Retarget one tab's provider (effective on its next launch), mirroring
    /// `ChangeAgentProvider` but scoped to a single tab. For the session-slot tab
    /// this delegates to the session-level change; for a
    /// extra tab it updates only that tab. `provider` is validated server-side.
    ChangeAgentTabProvider {
        session_id: String,
        tab_id: String,
        provider: String,
    },
    /// Remember the tab the user last focused on this agent, so switching away
    /// and back (either surface, and across restarts) restores it. `tab_id` of
    /// `None` (or equal to `session_id`, or naming a tab that doesn't belong to
    /// the session) clears the memory, resolving to the session-slot tab. High
    /// frequency and user-paced: dispatched with no status/toast (`status: None`)
    /// and the TUI/web writers treat it as fire-and-forget.
    SetLastFocusedTab {
        session_id: String,
        tab_id: Option<String>,
    },
    /// Switch a project's SOURCE checkout back to its default branch, mirroring
    /// the TUI's `checkout-project-default-branch`.
    ///
    /// The TUI runs this in two worker hops (an inspection job, then — only for
    /// the `Known` default-branch case — a `git switch` job), chained by an
    /// engine reaction the web loop does not act on. So the web does the
    /// inspection AND the checkout SYNCHRONOUSLY here (the engine loop already
    /// shells out to git synchronously for `PullProject`/discard), reproducing
    /// the TUI's four inspection outcomes byte-for-byte:
    ///   - default branch known and differs → `git switch`, then info.
    ///   - heuristic (origin/HEAD missing, on a non-main/master branch) → error.
    ///   - already on the leading branch → info, no checkout.
    ///   - inspection failed → error.
    ///
    /// The TUI does not confirm (it is a deliberate palette/keybinding action);
    /// the web confirms in the frontend dialog before sending this, since a ⋯
    /// menu click is a lighter gesture and the checkout moves HEAD.
    CheckoutProjectDefaultBranch {
        project_id: String,
    },
    /// Adopt an orphaned managed worktree (created by dux, no live session) as a
    /// new agent, mirroring the TUI's `new-agent-from-worktree`
    /// (`CreateAgentRequest::ExistingManagedWorktree`). `worktree_path` is the
    /// canonical path the listing returned; `name` is a DISPLAY name (the branch
    /// already exists, so this never becomes a branch — see the TUI's
    /// display-name prompt variant).
    ///
    /// The path is NEVER trusted from the client: `wire_to_command` re-runs the
    /// `classify_project_worktrees` classification for the project and rejects a
    /// path that isn't a currently-adoptable managed worktree (stale, foreign, or
    /// already attached). Classification is a `git worktree list` + branch lookups
    /// — bounded plumbing reads with no working-tree writes — so it runs inline in
    /// `wire_to_command` like `AddProject`'s `current_branch`/`leading_branch`
    /// inspection and `discard_classify`'s `git status` already do. (Contrast
    /// `CheckoutProjectDefaultBranch`, which `git switch`es the working tree and
    /// therefore goes through workers per the CLAUDE.md tenet.)
    CreateAgentFromWorktree {
        project_id: String,
        worktree_path: String,
        name: String,
    },
    /// Create a new agent checked out on a GitHub PR's head branch, mirroring
    /// the TUI's `new-agent-from-pr` palette command
    /// (`CreateAgentRequest::PullRequest`). `pr` is the user-typed reference: a
    /// full PR URL, `#123`, or a bare `123` (parsed server-side by
    /// `gh::parse_pull_request_lookup` against the project's GitHub remote).
    /// `name` is the agent/branch name; empty falls back to the PR head branch,
    /// matching the TUI prompt's default seed.
    ///
    /// The lookup shells out to `gh pr view`, so — like the TUI — this does NOT
    /// run inline: `apply_wire` validates the synchronous guards (gh available,
    /// known project, parseable input, valid name), spawns
    /// `gh::run_pull_request_lookup_job` off-thread, and returns a busy status.
    /// On success the worker posts `PullRequestResolved`, whose
    /// `OpenNewAgentPromptForPr` reaction the web actor's `drive_pr_lookup_followup`
    /// turns into a `CreateAgentRequest::PullRequest` dispatch (where the TUI
    /// would instead open a name prompt — the web already has the name). A
    /// lookup failure surfaces on the async status stream.
    CreateAgentFromPr {
        project_id: String,
        pr: String,
        name: String,
    },
    /// Run a configured text macro against a live PTY target, mirroring the TUI's
    /// macro bar (Ctrl-\). `target_id` names EITHER an agent session (an entry in
    /// `providers`, surface `Agent`) OR a companion terminal (an entry in
    /// `companion_terminals`, surface `Terminal`); the engine resolves which and
    /// unifies the write through the same `providers`-then-`companion_terminals`
    /// lookup the actor's `pty_for` uses. `name` is the macro's `[macros]` key.
    ///
    /// The engine resolves the entry by name (unknown → error), checks the
    /// macro's surface against the resolved target's surface (mismatch → error;
    /// the TUI bar simply doesn't list mismatches, but an explicit wire error is
    /// the right shape for a programmatic client), translates the text with the
    /// shared `dux_core::macros::macro_payload_bytes` (newlines → Alt+Enter), and
    /// writes it to the target's PTY. Parity with the TUI is by construction: the
    /// same transform, the same surface gate.
    RunMacro {
        target_id: String,
        name: String,
    },
    /// Wholesale-replace the `[macros]` config, mirroring the TUI macro editor's
    /// save semantics (the dialog rewrites the whole map). `entries` is an ORDERED
    /// list of `(name, {text, surface})` — order is preserved into the config
    /// IndexMap and thus into the ViewModel's `macros`. Persisted through the
    /// engine's config writer following the global-env precedent
    /// (`PersistGlobalEnv` → eager save through `Engine::config_writer`), so user
    /// comments survive the in-place patch. Validation (server-side, the
    /// client is never trusted): empty names rejected, duplicate names rejected,
    /// empty text rejected (the TUI editor refuses to save an empty-text macro),
    /// unknown surface strings rejected.
    UpdateMacros {
        entries: Vec<WireMacroEntry>,
    },
    /// Point the changed-files watch at a session's worktree, mirroring the TUI's
    /// selection-driven `reload_changed_files`. `session_id` is nullable: a
    /// `null` (or absent) id clears the watch so the global poller stops reading
    /// any worktree, while a real id has the server resolve the session and watch
    /// its worktree. The web sends this on every session selection because the
    /// global `watched_worktree`/`changed_files` engine state is otherwise never
    /// set for a browser client (only the TUI set it), leaving the pane empty.
    ///
    /// `#[serde(default)]` so the field accepts the absent form
    /// (`{"command":"watch_changed_files","args":{}}`) as well as an explicit
    /// `null`, matching the frontend's clear path.
    WatchChangedFiles {
        #[serde(default)]
        session_id: Option<String>,
    },
}

/// The curated favicon TINT color names accepted by `config.server.favicon` and
/// the customize-webapp dialog. The default (an empty value) is the original
/// full-color yellow duck; these names recolor a flat duck silhouette instead, so
/// `yellow` is intentionally NOT a tint. This is the CANONICAL list; the web
/// frontend mirrors it. Keep the two in sync.
pub const CURATED_FAVICON_COLORS: &[&str] = &[
    "violet", "blue", "sky", "cyan", "teal", "green", "amber", "orange", "red", "pink", "rose",
];

/// Normalize a favicon value for `config.server.favicon`.
///
/// Trims and lowercases the input. An empty (or whitespace-only) value resets the
/// favicon to the default (the empty string, which the web renders as the brand
/// yellow duck). A curated color name is accepted as-is. Any other value is
/// invalid and yields `None` so the caller can reject it.
pub fn normalize_instance_favicon(raw: &str) -> Option<String> {
    let normalized = raw.trim().to_lowercase();
    if normalized.is_empty() {
        return Some(String::new());
    }
    if CURATED_FAVICON_COLORS.contains(&normalized.as_str()) {
        Some(normalized)
    } else {
        None
    }
}

/// True for a character that must not survive into a title: a Unicode control
/// code (category Cc, via `char::is_control`) OR a bidi/format character (a subset
/// of category Cf) that can visually reorder or hide text. `char::is_control`
/// alone misses the Cf class, so a right-to-left override (U+202E) or zero-width
/// joiner would otherwise pass through and spoof the rendered tab title / wordmark
/// and the on-disk `config.toml` (a Trojan-Source-style display attack). We
/// neutralize the specific format characters that matter for spoofing.
fn is_unsafe_title_char(ch: char) -> bool {
    ch.is_control()
        || matches!(ch,
            // Zero-width + directional marks, bidi embeddings/overrides, bidi
            // isolates, and the byte-order mark / zero-width no-break space.
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}')
}

/// Normalize a title for `config.server.title`.
///
/// Replaces every unsafe character (Unicode control codes and bidi/format
/// characters — see [`is_unsafe_title_char`]) with a space, collapses runs of
/// whitespace to a single space, and trims. Caps the result to at most 200
/// characters (counted by `char`, never bytes, so multi-byte glyphs can't be
/// sliced mid-codepoint). An empty result resets to the default `"dux"` (which is
/// also the reset value the dialog sends as an empty title).
pub fn normalize_instance_title(raw: &str) -> String {
    let mut collapsed = String::new();
    let mut pending_space = false;
    for ch in raw.chars() {
        let ch = if is_unsafe_title_char(ch) { ' ' } else { ch };
        if ch == ' ' {
            // Defer emitting whitespace so runs collapse and leading space drops.
            if !collapsed.is_empty() {
                pending_space = true;
            }
        } else {
            if pending_space {
                collapsed.push(' ');
                pending_space = false;
            }
            collapsed.push(ch);
        }
    }
    let capped: String = collapsed.chars().take(200).collect();
    let trimmed = capped.trim();
    if trimmed.is_empty() {
        "dux".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Sanitize a `ui.terminal_font_family` settings-PATCH value: drop every ASCII
/// control character and cap the result to 200 characters (counted by `char`,
/// never bytes, so a multi-byte glyph can't be sliced mid-codepoint).
///
/// This is defense in depth alongside the web's own sanitizing
/// (`terminalFontFamily` in `crates/dux-web/web/src/lib/terminalFont.ts`),
/// which strips a wider set of characters because the value there is
/// concatenated directly into a CSS `font-family: <x>;` declaration and into a
/// `document.fonts.load` shorthand string. Here the goal is narrower: a stray
/// control character (in particular a literal newline) must never survive
/// into `config.toml`, and the field must not grow unbounded. Degrades
/// quietly rather than rejecting the patch, matching this settings API's
/// tolerant style elsewhere (e.g. [`normalize_instance_title`]).
pub fn sanitize_terminal_font_family(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_ascii_control())
        .take(200)
        .collect()
}

/// Clamp a numeric settings-PATCH field to `[0, max]`, preserving `0`
/// unclamped: several of these fields document `0` as a distinct "disable"/
/// "never" meaning (`status_clear_seconds`, `attention_grace_seconds`), not
/// the bottom of a numeric range, so the ceiling only applies to nonzero
/// input.
fn clamp_nonzero<T: PartialOrd + Copy>(value: T, max: T) -> T {
    if value > max { max } else { value }
}

/// Normalize a `ui.pr_banner_position` value for [`Engine::set_settings`].
/// Accepts exactly "top" or "bottom" (case-insensitive, trimmed); anything
/// else is rejected so the endpoint returns a plain-text 400 rather than
/// silently coercing an unrecognized value the way the parameterless toggle
/// does. Kept distinct from `toggle_pr_banner_position`'s "anything other than
/// bottom becomes top" leniency: that toggle only ever flips between the two
/// known values it already holds, while this validates a client-supplied
/// string.
pub fn normalize_pr_banner_position(raw: &str) -> Option<String> {
    match raw.trim().to_lowercase().as_str() {
        "top" => Some("top".to_string()),
        "bottom" => Some("bottom".to_string()),
        _ => None,
    }
}

/// The present/absent fields for [`WireCommand::SetSettings`] — the SINGLE
/// field list for the settings path, and the payload the variant carries.
///
/// Set explicit values for the "Settings" modal's `[ui]`/`[capabilities]`/
/// `[defaults]` knobs in one request, mirroring `SetInstanceIdentity`'s
/// all-optional present-field pattern but for the generic settings surface (the
/// Preferences dialog, opened from the app menu's cog; see
/// `crates/dux-web/web/src/lib/settingsDescriptors.ts` for the exact field set
/// and descriptions). Every field is optional; an absent field is left
/// untouched, and a patch with every field absent is a no-op (no write, no
/// `config.changed`). Numeric fields are clamped to a sane documented ceiling
/// (0 stays 0 where the config documents a zero meaning); `pr_banner_position`
/// and `default_provider` are validated against a fixed accepted set and the
/// whole command is REJECTED (no partial apply) if a present value is
/// unrecognized. `server.title` and `server.favicon` are deliberately NOT here,
/// they stay on `SetInstanceIdentity`. The write is eager so the endpoint can
/// report a synchronous success/failure, and a non-empty patch mutates
/// config-static state so the web fires `config.changed` and every tab refetches
/// its settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SettingsPatch {
    pub copy_on_select: Option<bool>,
    /// `ui.compose_bar`: WHEN the web's touch terminal puts up the compose bar
    /// (the buffered phone typing surface). One of `"auto"`, `"always"` or
    /// `"never"`; validated against that set like `pr_banner_position`, so an
    /// unrecognized value REJECTS the whole command rather than being coerced.
    /// A plain field write with no side effects otherwise.
    pub compose_bar: Option<String>,
    /// `ui.mobile_accessory_bar`: the web mobile terminal screens' accessory
    /// key bar (Esc/Tab/Ctrl/Alt/arrows). A plain field write with no side
    /// effects (a pure render gate), so it rides the generic settings path
    /// like `compose_bar`.
    pub mobile_accessory_bar: Option<bool>,
    /// `ui.upload_write_gitignore`: whether the agent upload directory keeps a
    /// self-ignoring `.gitignore`. A plain field write with no side effects
    /// (the next upload reads it), so it rides the generic settings path. Its
    /// companion `ui.upload_directory` is deliberately absent: it is a PATH,
    /// and the web has no directory picker to edit one with.
    pub upload_write_gitignore: Option<bool>,
    /// `ui.upload_pasted_text_chars`: how many characters a text paste onto an
    /// AGENT pane may run to before the web saves it as a file and pastes the
    /// path. A plain field write (the next paste reads it), so it rides the
    /// generic settings path. Out-of-range values are CLAMPED here rather than
    /// rejected, the same treatment `load_config` gives the on-disk value, so
    /// the two ways in cannot disagree about what a bad number means.
    pub upload_pasted_text_chars: Option<usize>,
    /// `ui.auto_reopen_agents`: the global startup auto-reopen switch. A plain
    /// field write with no side effects (the reopen pass reads it at the NEXT
    /// startup), so it rides the generic settings path.
    pub auto_reopen_agents: Option<bool>,
    pub show_changes_pane: Option<bool>,
    pub web_notifications: Option<bool>,
    pub always_show_tab_strip: Option<bool>,
    /// `ui.tab_reaches_agent`: whether the TUI's typeable center pane hands Tab
    /// and Shift-Tab to the agent. A plain field write the TUI reads live, so it
    /// rides the generic settings path even though nothing in the web reads it.
    pub tab_reaches_agent: Option<bool>,
    pub status_clear_seconds: Option<u16>,
    pub attention_grace_seconds: Option<u64>,
    pub attention_indicator: Option<bool>,
    pub attention_on_bell: Option<bool>,
    pub pr_banner_position: Option<String>,
    pub hyperlinks: Option<bool>,
    /// `defaults.enable_randomized_pet_name_by_default`. It rides the generic
    /// settings path (rather than its dedicated toggle endpoint) because
    /// flipping it is a plain field write with no side effects, unlike
    /// `ui.github_integration`, which arms/disarms PR sync and therefore keeps
    /// its own endpoint.
    pub enable_randomized_pet_name_by_default: Option<bool>,
    /// `defaults.provider`: the GLOBAL default provider for new agents in
    /// projects without a project-specific override, mirroring the TUI's
    /// `change-default-provider` palette command. Distinct from the per-project
    /// `ProjectConfig::default_provider` override, which has its own dedicated
    /// wire path and is out of scope here. Validated against the configured
    /// provider list (same source as `BootstrapView::available_providers`); an
    /// unrecognized name rejects the WHOLE patch, matching
    /// `pr_banner_position`.
    pub default_provider: Option<String>,
    /// `ui.disable_automated_welcome_screen`: suppresses the AUTOMATIC first-run
    /// welcome screen. A plain field write with no side effects (the gate reads
    /// it at the NEXT launch), so it rides the generic settings path. It does NOT
    /// disable the app menu's on-demand "Welcome screen…" entry, which bypasses
    /// the gate entirely.
    pub disable_automated_welcome_screen: Option<bool>,
    /// `ui.disable_release_notes`: suppresses the AUTOMATIC what's-new screen
    /// after a version change. Same plain-write reasoning as
    /// `disable_automated_welcome_screen`, and likewise leaves the on-demand
    /// "What's new…" menu entry working.
    pub disable_release_notes: Option<bool>,
    /// `ui.terminal_font_family`: a font name installed on the viewing device,
    /// placed ahead of the bundled web terminal font stack. Sanitized by
    /// [`crate::wire::sanitize_terminal_font_family`] before being stored:
    /// ASCII control characters are dropped and the result is capped to 200
    /// characters, defense in depth alongside the web `terminalFont.ts`
    /// sanitizing (the value is concatenated into a CSS `font-family`
    /// declaration client-side). Degrades quietly rather than rejecting the
    /// patch. Empty string is a valid value (it means "use the bundled stack
    /// only").
    pub terminal_font_family: Option<String>,
    /// `ui.terminal_font_size`: the web terminal's font size in pixels.
    /// Normalized through [`crate::config::normalized_terminal_font_size`]
    /// (pure: no warning here, since a value corrected here is a live PATCH
    /// from the Preferences dialog, not a value read off disk; see that
    /// function's doc comment), which resets an out-of-range value to the
    /// default rather than clamping to the nearer bound.
    pub terminal_font_size: Option<u16>,
    /// Presentation-only, NOT a settings field: when true, and the patch is
    /// confined to the accessory-bar field, the engine emits no info status
    /// for this request. That field gates chrome the user is looking
    /// straight at, so the bar visibly moving is the whole feedback and a
    /// "Settings updated." toast on top is noise. The flag is honored per
    /// request and only for that field (`is_accessory_bar_only`), so it can
    /// never silence any other settings write; validation and save errors
    /// still fail the command loudly regardless of the flag.
    #[serde(default)]
    pub quiet: bool,
}

impl SettingsPatch {
    /// True when the client sent at least one field.
    ///
    /// Every field is an `Option`, so "every field absent" is exactly
    /// `Self::default()`. That makes this a whole-struct compare rather than a
    /// per-field list, which means a field added to this struct CANNOT make it
    /// stale. The field-by-field version of this check is how a patch carrying
    /// only an unlisted field once read as empty and silently no-opped.
    pub fn any_present(&self) -> bool {
        // `quiet` is a presentation flag, not a settings field: neutralize it
        // in the compare so a patch carrying only `quiet` still reads as
        // empty (nothing to write, no `config.changed`).
        *self
            != Self {
                quiet: self.quiet,
                ..Self::default()
            }
    }

    /// True when the patch carries the accessory-bar field and no other
    /// settings field. This is the gate that scopes `quiet`: the whole-struct
    /// compare (every field its own default except that one and the flag)
    /// cannot go stale when a field is added, mirroring `any_present`.
    pub fn is_accessory_bar_only(&self) -> bool {
        self.mobile_accessory_bar.is_some()
            && *self
                == Self {
                    mobile_accessory_bar: self.mobile_accessory_bar,
                    quiet: self.quiet,
                    ..Self::default()
                }
    }
}

impl WireCommand {
    /// True for the commands that mutate config-static state surfaced in the
    /// bootstrap document — the macro set, the workspace-wide env map, the
    /// Changes-pane visibility flag, and the preference toggles. These
    /// save to `config.toml` (eager for github-integration, lazy for the
    /// low-stakes preferences) and adopt the change into the running config in
    /// place (no disk reload), so the web layer must fire a `config.changed`
    /// event after one succeeds for connected
    /// clients to refetch `/api/v1/bootstrap`. Without it the change persists but
    /// the UI keeps showing — and reseeds dialogs from — a stale snapshot (e.g. a
    /// just-saved macro appears to vanish). `ReloadConfig` is intentionally NOT
    /// listed: it re-reads the whole file and already signals through the engine
    /// actor's reload path.
    pub fn mutates_config_static(&self) -> bool {
        // An empty-body `SetInstanceIdentity` (both fields absent) never touches
        // config, so it must NOT trigger a `config.changed` fan-out. A body that
        // carries at least one field may mutate config (the handler still skips the
        // write when the values are unchanged, matching the sibling toggles).
        if let WireCommand::SetInstanceIdentity { title, favicon } = self {
            return title.is_some() || favicon.is_some();
        }
        // An empty-patch `SetSettings` (every field absent) never touches
        // config, mirroring the `SetInstanceIdentity` empty-body no-op above.
        if let WireCommand::SetSettings(patch) = self {
            return patch.any_present();
        }
        matches!(
            self,
            WireCommand::UpdateMacros { .. }
                | WireCommand::PersistGlobalEnv { .. }
                | WireCommand::SetChangesPaneVisible { .. }
                | WireCommand::ToggleRandomizedPetNameDefault {}
                | WireCommand::TogglePrBannerPosition {}
                | WireCommand::SetAgentSort { .. }
                | WireCommand::ToggleCopyOnSelect {}
                | WireCommand::ToggleGithubIntegration {}
                | WireCommand::ToggleAlwaysShowTabStrip {}
                | WireCommand::ToggleTabReachesAgent {}
                | WireCommand::SetTailscaleMode { .. }
        )
    }
}

/// A single macro in a [`WireCommand::UpdateMacros`] payload. `surface` is the
/// canonical lowercase string ("agent" | "terminal" | "both"), matching the
/// `MacroSurface` serde casing and the ViewModel's `MacroView::surface`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WireMacroEntry {
    pub name: String,
    pub text: String,
    pub surface: String,
}

/// A status-line update in wire-safe form.
///
/// Now also `Deserialize` so the serde `scope` default can be exercised (and so
/// a peer/older client's status JSON without a `scope` field round-trips to
/// [`StatusScope::All`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireStatus {
    /// "info" | "busy" | "warning" | "error"
    pub tone: String,
    pub message: String,
    /// `None` = an unkeyed transient (anonymous slot). `Some` = a keyed op whose
    /// later success/error/clear carries the same key so the surfaces correlate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Delivery audience for per-connection status filtering. Defaults to
    /// [`StatusScope::All`] (broadcast) when absent from the wire, so older
    /// peers / the TUI stay unaffected. The web's per-connection status
    /// forwarder delivers a status only when its scope is `All` or matches the
    /// connection's own id.
    #[serde(default)]
    pub scope: StatusScope,
    /// Whether the client must hold this toast until the user dismisses it,
    /// instead of retiring it on the usual timer. Defaults to `false` when
    /// absent from the wire. See
    /// [`dux_core::statusline::KeyedWireStatus::sticky`] for the rule.
    ///
    /// [`dux_core::statusline::KeyedWireStatus::sticky`]: crate::statusline::KeyedWireStatus::sticky
    #[serde(default)]
    pub sticky: bool,
}

impl WireStatus {
    /// Construct a wire status directly (for non-reaction sources like PTY-exit notices).
    pub fn new(tone: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tone: tone.into(),
            message: message.into(),
            key: None,
            scope: StatusScope::All,
            sticky: false,
        }
    }

    /// Construct a keyed wire status so producers can correlate updates.
    pub fn keyed(
        key: impl Into<String>,
        tone: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            tone: tone.into(),
            message: message.into(),
            key: Some(key.into()),
            scope: StatusScope::All,
            sticky: false,
        }
    }

    /// Builder that marks a status as one that waits for the user. See
    /// [`dux_core::statusline::KeyedWireStatus::sticky`] for when to use it.
    ///
    /// [`dux_core::statusline::KeyedWireStatus::sticky`]: crate::statusline::KeyedWireStatus::sticky
    pub fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }

    /// Builder that attaches a correlation key to an existing status.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Builder that sets the delivery [`StatusScope`] on an existing status.
    pub fn with_scope(mut self, scope: StatusScope) -> Self {
        self.scope = scope;
        self
    }

    fn from_update(update: &StatusUpdate) -> Self {
        Self {
            tone: update.tone.as_wire().to_string(),
            message: update.message.clone(),
            key: update.key.clone(),
            scope: update.scope.clone(),
            sticky: update.sticky,
        }
    }
}

/// What the client learns synchronously from applying a command. Fresh domain
/// state arrives separately via `view_model()`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct WireCommandOutcome {
    pub status: Option<WireStatus>,
    /// Whether a tab-close command (`KillSessionPty` for the session-slot tab,
    /// `CloseAgentTab` for an extra tab) DETACHED the agent. Computed in core
    /// with the in-flight-aware `any_tab_active`, so the web reads it directly
    /// instead of re-deriving detachment from `has_live_process` (a `providers`
    /// lookup that misses a sibling's in-flight launch). `None` for every other
    /// command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    /// The tab that took the session slot, set only when a `CloseAgentTab`
    /// closed the tab that HELD it. Nothing about the promoted tab changes but
    /// its role, so the browser uses this to land the user on it and to say
    /// which tab is the agent's first now, rather than resolving the slot
    /// against a spine that has not caught up with the close yet. `None` for an
    /// ordinary extra tab's close and for every other command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promoted: Option<String>,
    /// The opaque create-op id, set ONLY for a synchronous create dispatch
    /// (`CreateAgent` / `ForkSession` / `CreateAgentFromWorktree`). A REST create
    /// handler resolves its exact new session by polling
    /// [`Engine::created_session_for_op`] with this id, instead of a racy
    /// "first id not in the pre-snapshot" set-difference that could return a
    /// concurrent create's session. `None` for every other command and for the
    /// from-PR create (its op is minted later, inside the PR-lookup followup).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_op_id: Option<String>,
}

impl WireCommandOutcome {
    fn with_status(status: WireStatus) -> Self {
        Self {
            status: Some(status),
            ..Self::default()
        }
    }

    fn with_optional_status(status: Option<WireStatus>) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    fn with_detached(status: WireStatus, detached: bool) -> Self {
        Self {
            status: Some(status),
            detached: Some(detached),
            ..Self::default()
        }
    }

    fn with_close_tab(status: WireStatus, outcome: &CloseTabOutcome) -> Self {
        Self {
            status: Some(status),
            detached: Some(outcome.detached),
            promoted: outcome.promoted.as_ref().map(|t| t.as_str().to_string()),
            ..Self::default()
        }
    }

    fn quiet() -> Self {
        Self::default()
    }
}

/// Statuses produced by a web `drive_*_followup`, plus any keyed busies the
/// followup resolved to a `Final::Clear`. A `WireStatus` cannot represent a
/// clear (it has no "clear" tone — clearing is a separate `StatusEmitter::clear`
/// operation), so the followup hands the clear KEYS back for the web actor to
/// dismiss. The `statuses` are broadcast as usual.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebFollowupStatuses {
    pub statuses: Vec<WireStatus>,
    pub clear_keys: Vec<String>,
}

fn wire_status_from_reaction(reaction: &EventReaction) -> Option<WireStatus> {
    match reaction {
        EventReaction::Status(update) => Some(WireStatus::from_update(update)),
        EventReaction::Multi(items) => items.iter().find_map(wire_status_from_reaction),
        EventReaction::DeleteTerminalView(view) => view
            .label
            .as_ref()
            .map(|l| WireStatus::new("info", format!("Closed terminal \"{l}\"."))),
        _ => None,
    }
}

/// Named-field args for `finish_web_project_add`, so the two adjacent branch
/// strings (`current_branch`, `leading_branch`) can't be silently transposed at
/// a call site.
struct WebProjectAdd<'a> {
    path: &'a str,
    display_name: String,
    current_branch: &'a str,
    leading_branch: &'a str,
    status_message: String,
    /// Prefix for the "git succeeded but registration failed" error message.
    add_failed_prefix: &'a str,
    status_op_id: &'a Option<String>,
}

/// The authoritative "added" status message from a `PersistProject::Add`
/// reaction (the engine's real outcome — an honest dedup message when a race
/// hit that path, or the normal success text), or `None` for any other reaction.
fn added_status_message(reaction: &EventReaction) -> Option<String> {
    if let EventReaction::ProjectPersistenceOutcome(outcome) = reaction
        && let ProjectPersistenceView::Added { status_message, .. } = &outcome.view
    {
        Some(status_message.clone())
    } else {
        None
    }
}

/// Resolve a project's display name: the trimmed user-supplied name, or the
/// path's basename when empty. Shared by the add-project followups.
fn display_project_name(name: &str, path: &str) -> String {
    if name.trim().is_empty() {
        PathBuf::from(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string()
    } else {
        name.trim().to_string()
    }
}

/// Map an `EventReaction` to the user-facing status events it should emit on the
/// async status stream. Unlike `wire_status_from_reaction` (single value, for a
/// command's synchronous result), this flattens `Multi` and surfaces launch
/// failures, so background completions and failures reach web clients. The
/// messages mirror the TUI's `apply_agent_launch_failed_view` wording.
pub fn wire_statuses_from_reaction(reaction: &EventReaction) -> Vec<WireStatus> {
    match reaction {
        EventReaction::Status(update) => vec![WireStatus::from_update(update)],
        EventReaction::Multi(items) => items.iter().flat_map(wire_statuses_from_reaction).collect(),
        // Create-kind launch finals (success / startup-error / persist-fail /
        // launch-fail) are resolved ENGINE-SIDE against the shared
        // `Engine::pending_create_ops` op and ride alongside the launch View as a
        // sibling `Status` in the same `Multi`, surfaced by the `EventReaction::Status`
        // arm above. Reconnect / force-restart / resume-fallback / startup-auto-reopen
        // finals are resolved per-surface in `drive_web_launch_followup` against
        // `Engine::pending_web_launch_ops`. So both launch View reactions emit nothing
        // here, avoiding a double status.
        EventReaction::AgentLaunchFailedView(_) | EventReaction::AgentLaunchReadyView(_) => vec![],
        // DeleteTerminal is a one-shot info; no busy precedes it, so it stays
        // unkeyed (anonymous slot).
        EventReaction::DeleteTerminalView(view) => view
            .label
            .as_ref()
            .map(|l| WireStatus::new("info", format!("Closed terminal \"{l}\".")))
            .into_iter()
            .collect(),
        EventReaction::OpenConfigReloadFailedModal(message) => {
            vec![
                WireStatus::new("error", format!("Config reload failed: {message}"))
                    .with_key(status_keys::CONFIG_RELOAD),
            ]
        }
        EventReaction::ProjectWorktreesArrived {
            project_id,
            result: Err(message),
            ..
        } => vec![
            WireStatus::new("error", format!("Failed to list worktrees: {message}")).with_key(
                format!("{}:{project_id}", status_keys::WORKTREE_LIST_PREFIX),
            ),
        ],
        _ => vec![],
    }
}

/// User-facing message for a completed session deletion, varying by what
/// happened to the worktree.
pub fn delete_session_status_message(
    outcome: &FinishDeleteSessionOutcome,
    removal: &WorktreeRemoval,
) -> String {
    let name = outcome.session.display_label();
    match removal {
        // A standalone agent: dux's record of it is gone and the user's folder
        // is exactly as it was. Said explicitly rather than left implied,
        // because "Deleted agent X." on its own reads as though something on
        // disk went with it.
        WorktreeRemoval::NothingToRemove { folder_label } => {
            format!(
                "Deleted agent \"{name}\". Its folder \"{folder_label}\" was left untouched: \
                 dux never creates, moves or removes a standalone agent's folder. Anything \
                 the agent wrote there is still there."
            )
        }
        // The worktree went and the branches stayed, because they were not
        // dux's to delete. The wording names every kept branch with its own
        // reason (drift makes them differ) and how to remove one by hand: once
        // the worktree is gone, no dux surface can reach the branch any more.
        //
        // The `unwrap_or_default` on the branch reads below are unreachable
        // placeholders, not values any sentence renders: a standalone agent has
        // no branch and takes the `NothingToRemove` arm above, which is the
        // only arm it can take.
        WorktreeRemoval::Performed {
            branches: crate::engine::RemovedBranches::Kept(reason),
        } => {
            format!(
                "Deleted agent \"{name}\" and removed its worktree. {}",
                reason.kept_branches_note(
                    outcome.session.branch_name().unwrap_or_default(),
                    outcome.session.initial_branch().unwrap_or_default(),
                )
            )
        }
        WorktreeRemoval::Performed {
            branches: crate::engine::RemovedBranches::Deleted(branches),
        } => {
            let branch = outcome.session.branch_name().unwrap_or_default();
            let mut message = match &branches.branch {
                crate::git::BranchDeletion::Deleted => format!(
                    "Deleted agent \"{name}\", removed its worktree and deleted its branch \"{branch}\"."
                ),
                crate::git::BranchDeletion::AlreadyGone => format!(
                    "Deleted agent \"{name}\" and removed its worktree. Its branch \"{branch}\" was already gone."
                ),
                // A branch git REFUSED to delete is still there, so the line
                // must not claim otherwise: it names the branch, gives git's
                // reason, and says what the user can do about it.
                crate::git::BranchDeletion::Refused { reason } => format!(
                    "Deleted agent \"{name}\" and removed its worktree, but its branch \
                     \"{branch}\" is still there. {}",
                    crate::git::branch_refusal_note(branch, reason)
                ),
            };
            // Said only when the agent DRIFTED off the branch it was born on,
            // so the ordinary case still reads as one branch and the drifted
            // case never leaves the user guessing what became of the other.
            if let Some(note) =
                branches.initial_branch_note(outcome.session.initial_branch().unwrap_or_default())
            {
                message.push(' ');
                message.push_str(&note);
            }
            message
        }
        WorktreeRemoval::PreservedShared => {
            format!("Deleted agent \"{name}\". Worktree kept (shared with other agents).")
        }
        WorktreeRemoval::SkippedForSiblings => {
            format!("Deleted agent \"{name}\". Worktree kept (still used by other agents).")
        }
        WorktreeRemoval::PreservedOrphan => {
            format!("Deleted agent \"{name}\". Worktree left on disk.")
        }
    }
}

enum WireCommandMapping {
    Mapped(Command),
    Unhandled(WireCommand),
}

struct SettingsUiBehaviorPatch {
    copy_on_select: Option<bool>,
    compose_bar: Option<String>,
    mobile_accessory_bar: Option<bool>,
    upload_write_gitignore: Option<bool>,
    upload_pasted_text_chars: Option<usize>,
    auto_reopen_agents: Option<bool>,
    show_changes_pane: Option<bool>,
    always_show_tab_strip: Option<bool>,
    tab_reaches_agent: Option<bool>,
}

impl SettingsUiBehaviorPatch {
    fn apply_to(self, config: &mut crate::config::Config) {
        if let Some(value) = self.copy_on_select {
            config.ui.copy_on_select = value;
        }
        if let Some(value) = self.compose_bar {
            config.ui.compose_bar = value;
        }
        if let Some(value) = self.mobile_accessory_bar {
            config.ui.mobile_accessory_bar = value;
        }
        if let Some(value) = self.upload_write_gitignore {
            config.ui.upload_write_gitignore = value;
        }
        if let Some(value) = self.upload_pasted_text_chars {
            config.ui.upload_pasted_text_chars =
                crate::config::normalized_upload_pasted_text_chars(value);
        }
        if let Some(value) = self.auto_reopen_agents {
            config.ui.auto_reopen_agents = value;
        }
        if let Some(value) = self.show_changes_pane {
            config.ui.show_changes_pane = value;
        }
        if let Some(value) = self.always_show_tab_strip {
            config.ui.always_show_tab_strip = value;
        }
        if let Some(value) = self.tab_reaches_agent {
            config.ui.tab_reaches_agent = value;
        }
    }
}

struct SettingsUiDisplayPatch {
    status_clear_seconds: Option<u16>,
    attention_grace_seconds: Option<u64>,
    attention_indicator: Option<bool>,
    attention_on_bell: Option<bool>,
    pr_banner_position: Option<String>,
    disable_automated_welcome_screen: Option<bool>,
    disable_release_notes: Option<bool>,
    terminal_font_family: Option<String>,
    terminal_font_size: Option<u16>,
}

impl SettingsUiDisplayPatch {
    fn apply_to(self, config: &mut crate::config::Config) {
        if let Some(value) = self.status_clear_seconds {
            config.ui.status_clear_seconds =
                clamp_nonzero(value, crate::config::MAX_STATUS_CLEAR_SECONDS);
        }
        if let Some(value) = self.attention_grace_seconds {
            config.ui.attention_grace_seconds =
                clamp_nonzero(value, crate::config::MAX_ATTENTION_GRACE_SECONDS);
        }
        if let Some(value) = self.attention_indicator {
            config.ui.attention_indicator = value;
        }
        if let Some(value) = self.attention_on_bell {
            config.ui.attention_on_bell = value;
        }
        if let Some(value) = self.pr_banner_position {
            config.ui.pr_banner_position = value;
        }
        if let Some(value) = self.disable_automated_welcome_screen {
            config.ui.disable_automated_welcome_screen = value;
        }
        if let Some(value) = self.disable_release_notes {
            config.ui.disable_release_notes = value;
        }
        if let Some(value) = self.terminal_font_family {
            config.ui.terminal_font_family = sanitize_terminal_font_family(&value);
        }
        if let Some(value) = self.terminal_font_size {
            config.ui.terminal_font_size = crate::config::normalized_terminal_font_size(value);
        }
    }
}

struct SettingsDefaultsPatch {
    enable_randomized_pet_name_by_default: Option<bool>,
    default_provider: Option<String>,
}

impl SettingsDefaultsPatch {
    fn apply_to(self, config: &mut crate::config::Config) {
        if let Some(value) = self.enable_randomized_pet_name_by_default {
            config.defaults.enable_randomized_pet_name_by_default = value;
        }
        if let Some(value) = self.default_provider {
            config.defaults.provider = value;
        }
    }
}

struct SettingsCapabilitiesPatch {
    web_notifications: Option<bool>,
    hyperlinks: Option<bool>,
}

impl SettingsCapabilitiesPatch {
    fn apply_to(self, config: &mut crate::config::Config) {
        if let Some(value) = self.web_notifications {
            config.capabilities.web_notifications = value;
        }
        if let Some(value) = self.hyperlinks {
            config.capabilities.hyperlinks = value;
        }
    }
}

fn validate_settings_patch(
    mut patch: SettingsPatch,
    config: &crate::config::Config,
) -> anyhow::Result<SettingsPatch> {
    patch.pr_banner_position = patch
        .pr_banner_position
        .map(|raw| {
            normalize_pr_banner_position(&raw)
                .ok_or_else(|| anyhow::anyhow!("unknown PR banner position \"{raw}\""))
        })
        .transpose()?;
    patch.compose_bar = patch
        .compose_bar
        .map(|raw| {
            crate::config::ComposeBarMode::parse(&raw)
                .map(|mode| mode.as_str().to_string())
                .ok_or_else(|| anyhow::anyhow!("unknown compose bar mode \"{raw}\""))
        })
        .transpose()?;
    if let Some(provider) = &patch.default_provider
        && !config.providers.commands.contains_key(provider.as_str())
    {
        anyhow::bail!(
            "Provider \"{provider}\" is not configured. Pick one of the configured providers."
        );
    }
    Ok(patch)
}

impl Engine {
    /// Reconstruct and dispatch a wire command, returning a wire-safe outcome.
    pub fn apply_wire(&mut self, command: WireCommand) -> anyhow::Result<WireCommandOutcome> {
        // Stamp the synchronous command-result status with the current command
        // origin (set by the engine actor around `ApplyWire`). For the TUI and
        // every test, `current_origin` is `All`, so the status is unchanged.
        // Deferred busies/finals stamp themselves at their own mint sites (they
        // capture `current_origin` before their worker spawns), so they are not
        // re-stamped here.
        // Clear the create-op correlation slot before dispatch so the value we
        // read back reflects only THIS command's create (the engine actor is
        // single-threaded, so no concurrent command can set it in between).
        self.last_created_op_id = None;
        let mut outcome = self.apply_wire_inner(command)?;
        if let Some(status) = outcome.status.as_mut() {
            status.scope = self.current_origin.clone();
        }
        // Surface the create op id (set by a synchronous `DispatchCreateAgentRequest`
        // dispatch) so the REST create handler can correlate its exact new session.
        outcome.created_op_id = self.last_created_op_id.take();
        Ok(outcome)
    }

    fn apply_wire_inner(&mut self, command: WireCommand) -> anyhow::Result<WireCommandOutcome> {
        // Commands with direct engine methods or custom output shaping return
        // here; the rest use the shared `wire_to_command`/`apply` tail.
        match command {
            WireCommand::RenameSession { session_id, title } => {
                let status = self.rename_session(&session_id, &title)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::ReconnectSession { session_id, force } => {
                let status = self.reconnect_session(&session_id, force)?;
                return Ok(WireCommandOutcome::with_optional_status(status));
            }
            WireCommand::RerunStartupCommand { session_id } => {
                let status = self.rerun_startup_command(&session_id)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::CheckoutProjectDefaultBranch { project_id } => {
                let status = self.checkout_project_default_branch(&project_id)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::AddProjectCheckoutDefault { path, name } => {
                let status = self.add_project_checkout_default(&path, name)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::AddProjectCreateInitialCommit { path, name } => {
                let status = self.add_project_create_initial_commit(&path, name)?;
                return Ok(WireCommandOutcome::with_optional_status(status));
            }
            WireCommand::AddProjectInitRepo { path, name } => {
                let status = self.add_project_init_repo(&path, name)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::ChangeAgentProvider {
                session_id,
                provider,
            } => {
                let status = self.change_agent_provider_wire(&session_id, &provider)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::CreateAgentFromPr {
                project_id,
                pr,
                name,
            } => {
                let status = self.create_agent_from_pr(&project_id, &pr, name)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::SetChangesPaneVisible { visible } => {
                let status = self.set_changes_pane_visible(visible);
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::SetInstanceIdentity { title, favicon } => {
                let status = self.set_instance_identity(title, favicon)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::SetSettings(patch) => {
                // `set_settings` decides its own status presence: a quiet
                // accessory-bar-only patch succeeds with `None` (no toast).
                let status = self.set_settings(patch)?;
                return Ok(WireCommandOutcome::with_optional_status(status));
            }
            WireCommand::ToggleRandomizedPetNameDefault {} => {
                let status = self.toggle_randomized_pet_name_default();
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::TogglePrBannerPosition {} => {
                let status = self.toggle_pr_banner_position();
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::SetAgentSort { sort } => {
                let status = self.set_agent_sort(&sort);
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::ToggleCopyOnSelect {} => {
                let status = self.toggle_copy_on_select();
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::ToggleAlwaysShowTabStrip {} => {
                let status = self.toggle_always_show_tab_strip();
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::ToggleTabReachesAgent {} => {
                let status = self.toggle_tab_reaches_agent();
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::SetTailscaleMode { mode } => {
                self.set_tailscale_mode(&mode)?;
                // The serve layer reports the listener outcome; a status here
                // would produce two toasts for one gesture.
                return Ok(WireCommandOutcome::quiet());
            }
            WireCommand::ToggleGithubIntegration {} => {
                let status = self.toggle_github_integration();
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::RecheckGithub {} => {
                let update = self.request_gh_recheck();
                return Ok(WireCommandOutcome::with_status(WireStatus::from_update(
                    &update,
                )));
            }
            WireCommand::AttachPullRequest {
                session_id,
                host,
                owner_repo,
                number,
                title,
                state,
                url,
            } => {
                let message = self.apply_pr_attach(
                    &session_id,
                    &host,
                    &owner_repo,
                    number,
                    &title,
                    &state,
                    &url,
                )?;
                return Ok(WireCommandOutcome::with_status(WireStatus::new(
                    "info", message,
                )));
            }
            WireCommand::ClearPullRequestOverride { session_id } => {
                let message = self.clear_pull_request_override(&session_id)?;
                return Ok(WireCommandOutcome::with_status(WireStatus::new(
                    "info", message,
                )));
            }
            WireCommand::ResumePullRequestAutodetection { session_id } => {
                let message = self.resume_pr_autodetection(&session_id)?;
                return Ok(WireCommandOutcome::with_status(WireStatus::new(
                    "info", message,
                )));
            }
            WireCommand::KillSessionPty { session_id } => {
                let (status, detached) = self.kill_session_pty(&session_id)?;
                return Ok(WireCommandOutcome::with_detached(status, detached));
            }
            WireCommand::DetachAgent { session_id } => {
                let status = self.detach_agent(&session_id)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::CloseAgentTab { session_id, tab_id } => {
                let (status, outcome) = self.close_agent_tab_wire(&session_id, &tab_id)?;
                return Ok(WireCommandOutcome::with_close_tab(status, &outcome));
            }
            WireCommand::ChangeAgentTabProvider {
                session_id,
                tab_id,
                provider,
            } => {
                let status = self.change_tab_provider_wire(&session_id, &tab_id, &provider)?;
                return Ok(WireCommandOutcome::with_status(status));
            }
            WireCommand::SetLastFocusedTab { session_id, tab_id } => {
                self.set_last_focused_tab(&session_id, tab_id.as_deref())?;
                // Focus changes are high-frequency and user-paced, so they do
                // not surface a toast or status.
                return Ok(WireCommandOutcome::quiet());
            }
            WireCommand::AddProject { .. } => {
                // Add returns ProjectPersistenceOutcome rather than Status.
                // Surface Added explicitly and relay rollback errors verbatim.
                let core = self.wire_to_command(command)?;
                let reaction = self.apply(core)?;
                let status = match &reaction {
                    EventReaction::ProjectPersistenceOutcome(outcome) => match &outcome.view {
                        ProjectPersistenceView::Added { status_message, .. } => {
                            Some(WireStatus::new("info", status_message.clone()))
                        }
                        _ => wire_status_from_reaction(&reaction),
                    },
                    _ => wire_status_from_reaction(&reaction),
                };
                return Ok(WireCommandOutcome::with_optional_status(status));
            }
            _ => {}
        }
        let core = self.wire_to_command(command)?;
        let reaction = self.apply(core)?;
        let mut status = wire_status_from_reaction(&reaction);
        if status.is_none() {
            status = self.drive_delete_followup(&reaction).into_iter().next();
        }
        Ok(WireCommandOutcome::with_optional_status(status))
    }

    /// Persist the Changes (git) pane's visibility to `config.toml`
    /// (`ui.show_changes_pane`) so the choice survives a restart, mirroring the
    /// TUI's persist-on-toggle. The field is the single persisted source of
    /// truth: the next ViewModel broadcast carries it, and the web clears its
    /// optimistic override once they match. Visibility is a low-stakes
    /// preference, so the write is lazy: the queue coalesces concurrent toggles
    /// and logs any disk failure; no rollback is needed.
    fn set_changes_pane_visible(&mut self, visible: bool) -> WireStatus {
        // Idempotent: skip the disk write when nothing changes (also blunts a
        // client that spams the toggle).
        if self.config.ui.show_changes_pane == visible {
            let message = if visible {
                "Changes pane is already shown."
            } else {
                // The pane (and its actions menu) is already gone from the
                // screen, so this answer carries the same reopen directions as
                // the hide itself.
                "Changes pane is already hidden. Reopen it from the show button in the header, or set it permanently in Preferences."
            };
            return WireStatus::new("info", message.to_string());
        }
        self.config.ui.show_changes_pane = visible;
        self.config_writer.save_lazy(self.config.clone());
        let message = if visible {
            "Changes pane shown. Hide it again from the Changes actions menu, or set it permanently in Preferences."
        } else {
            "Changes pane hidden. Reopen it from the show button in the header, or set it permanently in Preferences."
        };
        WireStatus::new("info", message.to_string())
    }

    /// Persist this dux instance's identity — the browser tab title
    /// (`config.server.title`) and favicon color (`config.server.favicon`) — to
    /// `config.toml`, mirroring `toggle_github_integration`'s "persist a candidate
    /// first" pattern so the endpoint gets a synchronous success/failure before it
    /// replies `200`.
    ///
    /// Each field is optional. An empty body (both `None`) is a no-op — no write,
    /// no `config.changed`. A `Some` title is normalized (see
    /// [`normalize_instance_title`]); a `Some` favicon must be a curated color
    /// name or empty (see [`normalize_instance_favicon`]) or the command is
    /// rejected with an error, which the dispatch layer turns into a plain-text
    /// `400`. The write is eager and idempotent (unchanged values skip the disk
    /// write), and the command mutates config-static state so the web fires
    /// `config.changed` for connected clients to refetch their title + favicon.
    fn set_instance_identity(
        &mut self,
        title: Option<String>,
        favicon: Option<String>,
    ) -> anyhow::Result<WireStatus> {
        // Empty body: nothing to touch. Skip the disk write and the fan-out.
        if title.is_none() && favicon.is_none() {
            return Ok(WireStatus::new("info", "Nothing to update."));
        }
        let mut candidate = self.config.clone();
        if let Some(raw) = title {
            candidate.server.title = normalize_instance_title(&raw);
        }
        if let Some(raw) = favicon {
            candidate.server.favicon = normalize_instance_favicon(&raw)
                .ok_or_else(|| anyhow::anyhow!("unknown favicon color \"{}\"", raw))?;
        }
        // Idempotent: skip the write (and the fan-out) when nothing changed.
        if candidate.server.title == self.config.server.title
            && candidate.server.favicon == self.config.server.favicon
        {
            return Ok(WireStatus::new("info", "Instance identity unchanged."));
        }
        // Persist eagerly so a disk failure is surfaced before the endpoint
        // replies; only commit to the running config once the write succeeds.
        self.config_writer
            .save_eager(candidate.clone())
            .map_err(|err| anyhow::anyhow!("saving to config failed: {err}"))?;
        self.config.server.title = candidate.server.title;
        self.config.server.favicon = candidate.server.favicon;
        Ok(WireStatus::new(
            "info",
            "Instance name and favicon updated.",
        ))
    }

    /// Persist an explicit set of `[ui]`/`[capabilities]` settings-modal fields
    /// to `config.toml`, mirroring `set_instance_identity`'s "persist a
    /// candidate first" pattern. Every field in [`SettingsPatch`] is optional; an
    /// absent field is left untouched. Numeric fields are clamped into a sane
    /// documented ceiling (0 is preserved where the config documents a zero
    /// meaning, see each field's config.rs doc comment); `pr_banner_position`
    /// is validated against a fixed accepted set, and `default_provider`
    /// against the configured provider list, with the WHOLE command
    /// rejected (nothing written) if the present value is unrecognized,
    /// matching `set_instance_identity`'s all-or-nothing favicon validation.
    /// The write is eager and idempotent (unchanged values skip the
    /// disk write), and a non-empty patch mutates config-static state so the web
    /// fires `config.changed` for connected clients to refetch their settings.
    fn set_settings(&mut self, patch: SettingsPatch) -> anyhow::Result<Option<WireStatus>> {
        // Resolve the quiet flag before the destructure moves the patch
        // apart. Quiet drops the INFO statuses only, and only for a patch
        // confined to the accessory-bar field (see the field's doc); errors
        // below still bail loudly. `None` here means "succeeded, say
        // nothing" — the bar visibly moving is the feedback.
        let quiet = patch.quiet && patch.is_accessory_bar_only();
        let info = |status: WireStatus| if quiet { None } else { Some(status) };
        // Ask before the destructure moves the patch apart. This message is
        // deliberately distinct from "Settings unchanged." below: "you sent me
        // no fields" and "you sent values that already match" are different
        // statements, and both are pinned by tests.
        if !patch.any_present() {
            return Ok(info(WireStatus::new("info", "Nothing to update.")));
        }

        let patch = validate_settings_patch(patch, &self.config)?;
        // The exhaustive destructure keeps a newly-added settings field from
        // compiling until it is assigned to one of the domain patches below.
        let SettingsPatch {
            quiet: _,
            copy_on_select,
            compose_bar,
            mobile_accessory_bar,
            upload_write_gitignore,
            upload_pasted_text_chars,
            auto_reopen_agents,
            show_changes_pane,
            web_notifications,
            always_show_tab_strip,
            tab_reaches_agent,
            status_clear_seconds,
            attention_grace_seconds,
            attention_indicator,
            attention_on_bell,
            pr_banner_position,
            hyperlinks,
            enable_randomized_pet_name_by_default,
            default_provider,
            disable_automated_welcome_screen,
            disable_release_notes,
            terminal_font_family,
            terminal_font_size,
        } = patch;

        let mut candidate = self.config.clone();
        SettingsUiBehaviorPatch {
            copy_on_select,
            compose_bar,
            mobile_accessory_bar,
            upload_write_gitignore,
            upload_pasted_text_chars,
            auto_reopen_agents,
            show_changes_pane,
            always_show_tab_strip,
            tab_reaches_agent,
        }
        .apply_to(&mut candidate);
        SettingsUiDisplayPatch {
            status_clear_seconds,
            attention_grace_seconds,
            attention_indicator,
            attention_on_bell,
            pr_banner_position,
            disable_automated_welcome_screen,
            disable_release_notes,
            terminal_font_family,
            terminal_font_size,
        }
        .apply_to(&mut candidate);
        SettingsDefaultsPatch {
            enable_randomized_pet_name_by_default,
            default_provider,
        }
        .apply_to(&mut candidate);
        SettingsCapabilitiesPatch {
            web_notifications,
            hyperlinks,
        }
        .apply_to(&mut candidate);

        // Idempotent: skip the write (and the fan-out) when nothing changed
        // after clamping/normalization. `candidate` is a clone of `self.config`
        // with only the patched fields touched, so every other field compares
        // equal by construction and this whole-struct compare asks exactly the
        // question the old field-by-field chain asked: did any patched field
        // actually change? Unlike that chain, it cannot go stale when a field
        // is added.
        if candidate == self.config {
            return Ok(info(WireStatus::new("info", "Settings unchanged.")));
        }

        // Persist eagerly so a disk failure is surfaced before the endpoint
        // replies; only commit to the running config once the write succeeds.
        //
        // INVARIANT: `candidate` was cloned from `self.config` above and only
        // patched fields were mutated in between, so adopting it wholesale is a
        // no-op for every unpatched field. Keep the clone and this assign
        // adjacent: a `&mut self` call that mutated `self.config` between them
        // would be silently discarded here, so do not add one. Adjacency is the
        // only thing enforcing that, deliberately, since a discarded write
        // cannot be observed from outside and no test can catch it.
        //
        // The whole-struct assign is also what keeps memory in step with disk.
        // `save_eager` already persists all of `candidate`, so a field-by-field
        // copy-back can only make the running config adopt LESS than what was
        // just written. That asymmetry is how a value once landed on disk while
        // the engine kept serving the stale one until the next reload.
        self.config_writer
            .save_eager(candidate.clone())
            .map_err(|err| anyhow::anyhow!("saving to config failed: {err}"))?;
        self.config = candidate;
        Ok(info(WireStatus::new(
            "info",
            "Settings updated. Every connected browser picks up the change now; a \
             running dux TUI applies it after its next config reload or restart.",
        )))
    }

    /// Flip `defaults.enable_randomized_pet_name_by_default` and persist it,
    /// mirroring the TUI's `toggle-randomized-pet-name-default` handler. The
    /// server owns the value, so this reads-and-flips (no client-supplied bool);
    /// the write is lazy because it is a low-stakes preference.
    fn toggle_randomized_pet_name_default(&mut self) -> WireStatus {
        let next = !self.config.defaults.enable_randomized_pet_name_by_default;
        self.config.defaults.enable_randomized_pet_name_by_default = next;
        self.config_writer.save_lazy(self.config.clone());
        let message = if next {
            "Random pet-name default enabled. New agents start with a random pet name. Change it back in Preferences."
        } else {
            "Random pet-name default disabled. New agents start with an empty name. Change it back in Preferences."
        };
        WireStatus::new("info", message.to_string())
    }

    /// Flip `ui.pr_banner_position` between "top" and "bottom" and persist it,
    /// mirroring the TUI's `toggle-pr-banner-position` handler. Any value other
    /// than "bottom" is treated as "top" (so an unexpected/legacy string moves to
    /// "bottom" on first toggle). Low-stakes preference, lazy write.
    fn toggle_pr_banner_position(&mut self) -> WireStatus {
        let next = if self.config.ui.pr_banner_position == "bottom" {
            "top"
        } else {
            "bottom"
        };
        self.config.ui.pr_banner_position = next.to_string();
        self.config_writer.save_lazy(self.config.clone());
        WireStatus::new(
            "info",
            format!("PR banner moved to the {next} of the agent pane."),
        )
    }

    /// Set `ui.agent_sort` to an explicit, validated mode and persist it. Rejects
    /// unknown values (the web control only sends known ones, but the wire is a
    /// trust boundary). Low-stakes preference, lazy write.
    ///
    /// The shared value set has six modes: "active" (default), "updated",
    /// "created", "name" (ascending), "name_desc" (descending), and "manual" (the
    /// web's drag-reorder order). Each surface OFFERS its own subset in its picker
    /// but DISPLAYS any value the other set: the TUI cycles the five non-manual
    /// modes and the web sets active/updated/created/name/manual. This method is
    /// `pub` so the TUI's `sort-agents` palette command can drive it too.
    pub fn set_agent_sort(&mut self, sort: &str) -> WireStatus {
        const VALID: [&str; 6] = [
            "active",
            "updated",
            "created",
            "name",
            "name_desc",
            "manual",
        ];
        if !VALID.contains(&sort) {
            return WireStatus::new(
                "error",
                format!(
                    "Unknown agent sort \"{sort}\". Expected one of: {}.",
                    VALID.join(", ")
                ),
            );
        }
        if self.config.ui.agent_sort == sort {
            return WireStatus::new("info", format!("Agent sort is already \"{sort}\"."));
        }
        self.config.ui.agent_sort = sort.to_string();
        self.config_writer.save_lazy(self.config.clone());
        WireStatus::new("info", format!("Agent list now sorted by \"{sort}\"."))
    }

    /// Save `[server] tailscale` as `mode`, refusing anything outside the
    /// tri-state. The write only: the running listener is moved by the serve
    /// layer, which is the only thing that owns the Tailscale leg.
    ///
    /// Lazy rather than eager, matching the sibling preferences: the serve has
    /// already acted by the time the file lands, and the saved value only decides
    /// what the NEXT run does.
    pub fn set_tailscale_mode(
        &mut self,
        mode: &str,
    ) -> anyhow::Result<crate::config::TailscaleMode> {
        let parsed = crate::config::TailscaleMode::parse(mode).ok_or_else(|| {
            anyhow::anyhow!("Unknown Tailscale mode \"{mode}\". Expected one of: auto, yes, no.")
        })?;
        if self.config.server.tailscale_mode() != parsed {
            self.config.server.tailscale = parsed.as_str().to_string();
            self.config_writer.save_lazy(self.config.clone());
        }
        Ok(parsed)
    }

    /// Flip `ui.copy_on_select` and persist it, mirroring the web
    /// `toggle-copy-on-select` palette command. Low-stakes preference, lazy write.
    fn toggle_copy_on_select(&mut self) -> WireStatus {
        let next = !self.config.ui.copy_on_select;
        self.config.ui.copy_on_select = next;
        self.config_writer.save_lazy(self.config.clone());
        let message = if next {
            "Copy-on-select enabled. Selecting terminal text now copies it to the clipboard."
        } else {
            "Copy-on-select disabled. Use Ctrl-Shift-c, the right-click menu, or Ctrl-Insert to copy."
        };
        WireStatus::new("info", message.to_string())
    }

    /// Flip `ui.always_show_tab_strip` and persist it, mirroring the TUI's
    /// `toggle-always-show-tabs` palette command. Low-stakes preference, lazy write.
    fn toggle_always_show_tab_strip(&mut self) -> WireStatus {
        let next = !self.config.ui.always_show_tab_strip;
        self.config.ui.always_show_tab_strip = next;
        self.config_writer.save_lazy(self.config.clone());
        let message = if next {
            "Agent tab strip is now always shown, even with a single tab. Change it back in Preferences."
        } else {
            "Agent tab strip now shows only once an agent has two or more tabs. Change it back in Preferences."
        };
        WireStatus::new("info", message.to_string())
    }

    /// Flip `ui.tab_reaches_agent` and persist it, mirroring the TUI's
    /// `toggle-tab-to-agent` palette command. Low-stakes preference, lazy write.
    fn toggle_tab_reaches_agent(&mut self) -> WireStatus {
        let next = !self.config.ui.tab_reaches_agent;
        self.config.ui.tab_reaches_agent = next;
        self.config_writer.save_lazy(self.config.clone());
        let message = if next {
            "Tab and Shift-Tab now reach the agent in the terminal app's center pane. The pane chords still move between panes. Change it back in Preferences."
        } else {
            "Tab moves between panes again in the terminal app. Change it back in Preferences."
        };
        WireStatus::new("info", message.to_string())
    }

    /// Flip `ui.github_integration` and persist it, mirroring the TUI's
    /// `toggle-github-integration` handler. Besides the config flag, this drives
    /// the engine's PR-sync side effects so the running server actually starts or
    /// stops polling `gh`. Disabling clears cached PR statuses and disarms the
    /// sync; ENABLING launches the `gh` host probe and does nothing else, because
    /// the status it holds right now predates that probe by definition. The
    /// probe's completion is what arms the work, exactly once. The eager save
    /// runs FIRST: if it fails nothing is mutated, so the engine never ends up
    /// half-changed against a config that did not persist.
    fn toggle_github_integration(&mut self) -> WireStatus {
        let next = !self.github_integration_enabled;
        let state = if next { "enabled" } else { "disabled" };
        // Persist a candidate config first; only mutate runtime state + PR sync
        // once the write succeeds.
        let mut candidate = self.config.clone();
        candidate.ui.github_integration = next;
        if let Err(err) = self.config_writer.save_eager(candidate) {
            let verb = if next { "enable" } else { "disable" };
            return WireStatus::new(
                "error",
                format!("Could not {verb} GitHub integration: saving to config failed: {err}"),
            );
        }
        self.github_integration_enabled = next;
        self.config.ui.github_integration = next;
        if next {
            // Off-to-on: re-ask `gh` which hosts it can serve, and do NOTHING
            // else. Without the re-ask the host policy would stay whatever it
            // was when the server booted, so a user who logs in to their
            // enterprise host and then enables the integration would get an
            // empty eligible set and no explanation. And acting on
            // `self.gh_status` here would be acting on the answer this probe is
            // about to replace: it launched a refresh immediately, the probe's
            // completion launched a second one plus another poller, and doing it
            // twice multiplied the API traffic again.
            self.spawn_gh_status_check();
        } else {
            self.pr_statuses.clear();
            self.disarm_pr_sync();
        }
        let detail = if next {
            "dux now syncs pull-request status in the background. Turn it off again in Preferences."
        } else {
            "Background PR syncing stopped and cached statuses cleared. Turn it back on in Preferences."
        };
        WireStatus::new("info", format!("GitHub integration {state}. {detail}"))
    }

    /// Force-kill one running agent's PTY without deleting its session or
    /// worktree. Mirrors the force-reconnect teardown block (drop the provider →
    /// SIGKILL on Drop, clear resume state) but stops short of relaunching, and
    /// marks the session Detached so it can be reconnected later. The per-tick
    /// spine diff notices the status change, rebuilds the workspace document and
    /// pushes it, so connected clients show the agent as detached. Unknown session
    /// is an `Err` (the REST handler, `delete_tab`'s session-slot branch, maps
    /// this method's `Err` to 400 — an unknown session is already caught earlier
    /// by `resolve_worktree`'s 404); an agent that is not running is an
    /// idempotent no-op.
    fn kill_session_pty(&mut self, session_id: &str) -> anyhow::Result<(WireStatus, bool)> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        // The teardown decision (SIGKILL the provider, clear every runtime map
        // including the in-flight `AgentLaunch` key, detach-if-last, and clear
        // `desired_running` on detach so startup auto-reopen does not relaunch a
        // deliberately-killed agent) is the single-source `kill_tab_runtime`,
        // shared with the TUI kill overlay so both surfaces agree.
        let outcome = self.kill_tab_runtime(session.slot_tab_id().as_str());
        if !outcome.killed {
            // No live PTY -> nothing to kill. Idempotent success so a
            // double-click or a kill racing a natural exit is not an error. The
            // agent is detached iff nothing of it is live now.
            let detached = !self.any_tab_active(&session.id);
            return Ok((
                WireStatus::new(
                    "info",
                    format!("Agent \"{}\" is not running.", session.display_label()),
                ),
                detached,
            ));
        }
        if !outcome.detached {
            // A live sibling tab kept the agent attached: this just closed one
            // tab and the agent stays Active in the sidebar.
            return Ok((
                WireStatus::new(
                    "info",
                    format!(
                        "Closed a tab for agent \"{}\". Its other tabs are still running.",
                        session.display_label()
                    ),
                ),
                false,
            ));
        }
        Ok((
            WireStatus::new(
                "info",
                format!(
                    "Closed the last tab for agent \"{}\". It is now detached — reconnect it from the agent menu.",
                    session.display_label()
                ),
            ),
            true,
        ))
    }

    /// Detach an agent WHOLE: stop every one of its tabs' provider processes and
    /// mark the session Detached. Unlike `kill_session_pty` (which stops a single
    /// tab and detaches only when it was the last live one), this is the explicit
    /// agent-level "Detach agent" action and always tears down every tab. The row
    /// and worktree are kept so the agent can be reconnected; `desired_running` is
    /// cleared so startup auto-reopen does not relaunch it. Unknown session is an
    /// `Err`; an agent with no live tabs is an idempotent no-op.
    fn detach_agent(&mut self, session_id: &str) -> anyhow::Result<WireStatus> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        let tabs = self.tab_ids_for_session(&session.id);
        let stopped = tabs
            .iter()
            .filter(|tab_id| self.providers.contains_key(*tab_id))
            .count();
        // Clear every tab's runtime maps (drops each live provider on the way,
        // SIGKILL on Drop), whether or not it was live, so no stale entry leaks.
        for tab_id in &tabs {
            self.clear_tab_runtime(tab_id);
        }
        if stopped == 0 {
            return Ok(WireStatus::new(
                "info",
                format!("Agent \"{}\" is not running.", session.display_label()),
            ));
        }
        self.mark_session_status(&session.id, crate::model::SessionStatus::Detached);
        self.mark_session_desired_running(&session.id, false);
        Ok(WireStatus::new(
            "info",
            format!(
                "Detached agent \"{}\" and stopped {} tab{}. It stays in Projects — reconnect it from the agent menu.",
                session.display_label(),
                stopped,
                if stopped == 1 { "" } else { "s" },
            ),
        ))
    }

    /// Rename an agent session's display title, mirroring the title half of the
    /// TUI's `apply_rename_session`. The custom `title` is trimmed; a non-empty
    /// title is validated with the same `is_valid_agent_name` backstop the TUI
    /// enforces and stored as `Some(title)`; an empty title clears it back to
    /// `None` so the row reverts to the branch name.
    ///
    /// Deliberate deviation from the TUI prompt: that prompt also renames the
    /// git branch by default (a `rename_branch` checkbox) and rejects an empty
    /// name outright. The web rename is title-only — it never touches the git
    /// branch — so clearing the title is the only way to revert to the branch
    /// name, which is why an empty title clears rather than errors here.
    fn rename_session(&mut self, session_id: &str, title: &str) -> anyhow::Result<WireStatus> {
        let trimmed = title.trim();
        // The refname rules apply only where the name can become a git branch.
        // A STANDALONE agent's title is a label taken verbatim at creation
        // (folder names legally contain characters a ref cannot), so validating
        // it as a ref here made a title dux itself minted impossible to type
        // back. Same rule as `Engine::prepare_branch_rename`.
        let branch_named = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.supports_branch_git())
            .unwrap_or(true);
        let new_title = if trimmed.is_empty() {
            None
        } else {
            if branch_named && !crate::git::is_valid_agent_name(trimmed) {
                anyhow::bail!(
                    "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                     underscores and slashes; it must start with a letter or digit, must \
                     not contain \"//\", and must not end with \"/\"."
                );
            }
            Some(trimmed.to_string())
        };
        let session = self
            .sessions
            .iter_mut()
            .find(|s| s.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        session.title = new_title.clone();
        session.updated_at = chrono::Utc::now();
        // Re-borrow immutably to persist (the mutable borrow above has ended).
        if let Some(session) = self.sessions.iter().find(|s| s.id == session_id) {
            self.session_store.upsert_session(session)?;
        }
        let message = match &new_title {
            Some(name) => format!("Renamed agent to \"{name}\"."),
            None => {
                // A standalone agent has no branch to fall back to; its label
                // falls back to its folder's name instead, so the sentence has
                // to name whichever one it actually landed on.
                let session = self.sessions.iter().find(|s| s.id == session_id);
                match session.and_then(|s| s.branch_name().map(str::to_string)) {
                    Some(branch) => format!(
                        "Cleared the custom name. Agent shows its branch name \"{branch}\" again."
                    ),
                    None => {
                        let label = session
                            .map(|s| s.display_label())
                            .unwrap_or_else(|| session_id.to_string());
                        format!(
                            "Cleared the custom name. Agent shows its folder's name \"{label}\" again."
                        )
                    }
                }
            }
        };
        Ok(WireStatus::new("info", message))
    }

    /// Swap which provider a session uses, mirroring the TUI's
    /// `apply_change_agent_provider`. The engine half (persist + pin) lives in
    /// [`Engine::change_agent_provider`]; this wire wrapper validates the
    /// provider against the configured provider list (the ViewModel's
    /// `available_providers` source — never trusting the client), handles the
    /// no-op "already uses this provider" case, and formats the status message.
    ///
    /// Deliberate substitution: the TUI's messages reference the rebindable
    /// `reconnect-agent` keybinding label ("press {key} to relaunch"); the web
    /// has no keybindings, so it points the user at the agent's Reconnect action
    /// instead, while keeping the rest of the wording byte-identical.
    fn change_agent_provider_wire(
        &mut self,
        session_id: &str,
        provider: &str,
    ) -> anyhow::Result<WireStatus> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        let label = session.display_label();
        let current = session.provider.clone();

        // Validate against the configured provider list — the same source the
        // ViewModel's `available_providers` is built from — so a forged or
        // stale provider name from the client is rejected with actionable copy.
        if !self.config.providers.commands.contains_key(provider) {
            anyhow::bail!(
                "Provider \"{provider}\" is not configured. Pick one of the configured providers."
            );
        }
        let provider = ProviderKind::new(provider);

        // No-op when the session already uses the chosen provider (mirrors the
        // TUI's `is_current` short-circuit, which knows the display label).
        if provider == current {
            return Ok(WireStatus::new(
                "info",
                format!(
                    "Agent \"{}\" already uses {}. Pick another provider to swap.",
                    label,
                    provider.as_str(),
                ),
            ));
        }

        let outcome = self.change_agent_provider(session_id, provider.clone())?;

        if outcome.running {
            Ok(WireStatus::new(
                "warning",
                format!(
                    "Worktree \"{}\" is set to {}, but the {} agent is still running. Exit it and reconnect the agent to relaunch with {}.",
                    label,
                    provider.as_str(),
                    outcome.previous.as_str(),
                    provider.as_str(),
                ),
            ))
        } else {
            let resume_note = if outcome.resume_available {
                " dux will resume its prior session on this worktree."
            } else {
                " This provider hasn't run on this worktree yet, so it'll start a fresh session."
            };
            Ok(WireStatus::new(
                "info",
                format!(
                    "Worktree \"{}\" will use {} next launch. Reconnect the agent to start it.{}",
                    label,
                    provider.as_str(),
                    resume_note,
                ),
            ))
        }
    }

    /// Close a tab and produce a confirming status, handing the caller the whole
    /// [`CloseTabOutcome`] so the route can report both halves of it.
    ///
    /// Both names are read BEFORE the close, from the strip the user was looking
    /// at when they asked. That is the strip the confirmation named its
    /// successor from, so a toast built the same way cannot contradict the
    /// dialog that raised it, and the closed tab's own row is still there to be
    /// named.
    fn close_agent_tab_wire(
        &mut self,
        session_id: &str,
        tab_id: &str,
    ) -> anyhow::Result<(WireStatus, CloseTabOutcome)> {
        let sid = SessionIdRef::new(session_id);
        let closed = self.tab_prose_label(sid, TabIdRef::new(tab_id));
        let successor = self
            .successor_slot_tab(sid)
            .map(|t| TabId::new(t.id.clone()))
            .and_then(|id| self.tab_prose_label(sid, id.as_ref_id()));
        let outcome = self.close_tab(session_id, tab_id)?;
        let message = match (&outcome.promoted, closed.as_deref(), successor.as_deref()) {
            (Some(_), Some(closed), Some(next)) => format!(
                "Closed the first tab, {closed}. {next} took its place as the agent's first tab."
            ),
            (Some(_), None, Some(next)) => format!(
                "Closed the agent's first tab. {next} took its place as the agent's first tab."
            ),
            (_, Some(closed), _) => format!("Closed the {closed} tab."),
            (_, None, _) => "Closed the tab.".to_string(),
        };
        Ok((WireStatus::new("info", message), outcome))
    }

    /// Retarget one tab's provider, validating the choice server-side. The
    /// session-slot tab delegates to the session-level change (identical UX); an
    /// extra tab updates only that tab.
    fn change_tab_provider_wire(
        &mut self,
        session_id: &str,
        tab_id: &str,
        provider: &str,
    ) -> anyhow::Result<WireStatus> {
        // Two adjacent wire path segments: named at the door, before a slot-ness
        // question that would otherwise accept them in either order.
        if self.is_slot_tab_of(SessionIdRef::new(session_id), TabIdRef::new(tab_id)) {
            return self.change_agent_provider_wire(session_id, provider);
        }
        if !self.config.providers.commands.contains_key(provider) {
            anyhow::bail!(
                "Provider \"{provider}\" is not configured. Pick one of the configured providers."
            );
        }
        let provider = ProviderKind::new(provider);
        let outcome = self.change_tab_provider(session_id, tab_id, provider.clone())?;
        if outcome.running {
            Ok(WireStatus::new(
                "warning",
                format!(
                    "This tab is set to {}, but the {} process is still running. Close and reopen the tab to relaunch with {}.",
                    provider.as_str(),
                    outcome.previous.as_str(),
                    provider.as_str(),
                ),
            ))
        } else {
            Ok(WireStatus::new(
                "info",
                format!(
                    "This tab will use {} on its next launch (it starts fresh).",
                    provider.as_str(),
                ),
            ))
        }
    }

    /// Reconnect (relaunch) an agent session's provider. Mirrors the TUI's
    /// `reconnect_selected_session` (`force == false`) and `force_reconnect_agent`
    /// (`force == true`):
    ///
    /// - Both require the session to exist and its worktree to still be present.
    /// - Normal reconnect REFUSES while a provider is already connected
    ///   (matching the TUI's "already connected" early-return); force reconnect
    ///   first tears down any running provider + pins + activity + resume
    ///   candidate, then starts fresh with no resume args.
    /// - Normal reconnect resumes the prior conversation per
    ///   `tab_resume_decision` (the same per-provider liveness check every other
    ///   launch path uses); force never resumes.
    ///
    /// Deliberate substitution: the TUI sources the PTY size from view state
    /// (`last_pty_size`); the web has no such state, so it uses the same default
    /// `(24, 80)` the subscribe-launch path already uses (`launch_agent`). The
    /// focused TerminalPane re-attaches via the existing subscribe machinery
    /// once the new provider comes up.
    ///
    /// Returns the busy status to surface synchronously (matching the TUI's
    /// `set_busy` after a successful dispatch), or the launch view's status when
    /// the dispatch was refused (e.g. a launch already in flight).
    fn reconnect_session(
        &mut self,
        session_id: &str,
        force: bool,
    ) -> anyhow::Result<Option<WireStatus>> {
        // The guards, the collision-aware resume decision, the status message,
        // and the pre-dispatch mutations (force teardown + conflicting-worktree
        // detach) are the single-source `Engine::reconnect_plan`, shared with the
        // TUI. The web sources no PTY size, so it uses the subscribe-launch
        // default `(24, 80)` (rows, cols); the focused pane re-attaches via the
        // subscribe machinery once the new provider comes up.
        let (request, busy) = match self.reconnect_plan(session_id, force, (24, 80))? {
            crate::engine::ReconnectPlan::AlreadyConnected { message } => {
                return Ok(Some(WireStatus::new("info", message)));
            }
            // The TUI surfaces this as a status error; the web surfaces it as a
            // 400 (its route maps `Err` to a bad-request body), preserving today's
            // behavior and message.
            crate::engine::ReconnectPlan::WorktreeMissing { message } => {
                anyhow::bail!(message);
            }
            crate::engine::ReconnectPlan::Launch {
                request,
                busy_message,
                ..
            } => (request, busy_message),
        };
        let reaction = self.apply(Command::DispatchAgentLaunch { request })?;
        // `DispatchAgentLaunch` returns a `DispatchAgentLaunchView`, which
        // `wire_status_from_reaction` doesn't surface. Mirror the TUI: on a
        // successful dispatch (`launched`) show the busy status; otherwise
        // surface the view's own status (e.g. "already launching").
        match reaction {
            EventReaction::DispatchAgentLaunchView(view) => {
                if view.launched {
                    // Mint the web reconnect/force-restart op: its opaque id
                    // correlates this busy to the final resolved when the launch
                    // reports back (in `drive_web_launch_followup`). The web
                    // counterpart to the TUI's `App.pending_reconnect_ops`. Stashed
                    // by session id (the launch completion carries the session).
                    let op = crate::engine::status_op(busy)
                        .resolve_in_handler(|o: &crate::engine::LaunchOutcome| {
                            crate::engine::launch_outcome_final(o)
                        })
                        .with_scope(self.current_origin.clone());
                    let pending = WireStatus::from_update(&self.begin_status_op(&op));
                    self.pending_web_launch_ops
                        .insert(session_id.to_string(), op);
                    Ok(Some(pending))
                } else {
                    Ok(view.status.as_ref().map(WireStatus::from_update))
                }
            }
            other => Ok(wire_status_from_reaction(&other)),
        }
    }

    /// Re-run the agent's project startup command in that agent's worktree.
    /// Mirrors the TUI's `rerun_startup_command_on_agent`: resolve the session and
    /// its project, require a non-empty project startup command, merge global +
    /// project env, then dispatch the blocking run off-thread via `spawn_status_op`
    /// so a keyed Busy is shown now and replaced by the same-key success/failure
    /// final when the command finishes. Returns the pending Busy as the wire
    /// outcome; unknown session/project or a missing startup command is an `Err`
    /// (surfaced by the REST handler as a 400 with the message).
    fn rerun_startup_command(&mut self, session_id: &str) -> anyhow::Result<WireStatus> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        // A startup command is a project's worktree-provisioning step, so a
        // standalone agent has neither a project to take one from nor a
        // worktree to provision. Refused with the shape of the thing rather
        // than "could not find the project", which would suggest a missing
        // record instead of a feature that does not apply.
        let managed = self
            .branch_git_workspace(
                session_id,
                "run a startup command for",
                "A startup command provisions a new worktree, and this agent runs in a folder that already exists. Add its folder as a project if you want dux to provision worktrees for it.",
            )?
            .clone();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == managed.project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Could not find the selected agent's project."))?;
        let command = project
            .startup_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Project \"{}\" does not have a startup command.",
                    project.name
                )
            })?;

        let paths = self.paths.clone();
        let terminal = self.config.startup_command_terminal.clone();
        let env =
            crate::config::resolve_agent_env(&self.config.env, &project.env).unwrap_or_default();
        let branch = managed.branch_name.clone();
        let success_name = project.name.clone();
        let failure_name = project.name.clone();
        let op = crate::engine::status_op(format!(
            "Rerunning startup command for agent \"{branch}\"..."
        ))
        .on_success(move |_: &()| {
            crate::engine::Final::info(format!(
                "Startup command completed for project \"{success_name}\". Open the agent's startup command logs to view the latest run."
            ))
        })
        .on_failure(move |err: &String| {
            // STICKY, for the same reason as the agent-side startup failure in
            // `command.rs`: provisioning stopped part-way, so the worktree is in
            // an unknown state, and the message sends the user to the logs.
            crate::engine::Final::error(format!(
                "Startup command failed for project \"{failure_name}\": {err}. Open the startup command logs for details."
            ))
            .sticky()
        });
        let run = crate::startup::StartupCommandRun {
            project,
            session,
            managed,
            command,
            terminal,
            env,
        };
        let reaction = self.spawn_status_op(op, move || {
            crate::startup::run_startup_command(&paths, run).status
        });
        // `spawn_status_op` returns the pending Busy as an `EventReaction::Status`;
        // surface it as the wire outcome so the originating client shows the spinner
        // (the same-key final follows over the status stream when the run ends).
        Ok(wire_status_from_reaction(&reaction).unwrap_or_else(|| {
            WireStatus::new(
                "info",
                format!("Rerunning startup command for agent \"{branch}\"..."),
            )
        }))
    }

    /// Switch a project's source checkout back to its default branch, mirroring
    /// the TUI's `checkout_selected_project_default_branch` (sessions.rs).
    ///
    /// This kicks off the TUI's two-worker chain rather than doing the work
    /// inline: `git switch` rewrites the working tree (seconds on large repos,
    /// blocks indefinitely on a held `index.lock`), and the web engine loop
    /// thread also drives every ViewModel push and PTY route, so a synchronous
    /// checkout would freeze every connected browser. The CLAUDE.md workers
    /// tenet forbids that.
    ///
    /// Worker 1 (`run_checkout_project_default_branch_inspection_job`) inspects
    /// the branch off-thread and posts `CheckoutProjectDefaultBranchInspected`.
    /// `process_worker_event` turns that into either a `Status` (heuristic /
    /// already-leading / inspection-error — surfaced by the actor's existing
    /// `wire_statuses_from_reaction` drain) or a
    /// `DispatchProjectDefaultBranchCheckout` reaction for the Known case, which
    /// `drive_checkout_followup` picks up to spawn worker 2 (the actual switch).
    ///
    /// Returns the busy status to surface synchronously (matching the TUI's
    /// `set_busy`). `Err` is reserved for the cheap project lookup / missing
    /// path guards (mirroring `PullProject`), so every real outcome comes back
    /// later as an async status.
    fn checkout_project_default_branch(&mut self, project_id: &str) -> anyhow::Result<WireStatus> {
        // Mirror the TUI's `checkout_selected_project_default_branch` guards:
        // resolve the project and refuse when its checkout path is missing.
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
        if project.path_missing {
            anyhow::bail!(
                "Cannot check out default branch: path not found for \"{}\"",
                project.name
            );
        }

        // Spawn worker 1 exactly as the TUI does, with the same busy message.
        let busy = format!(
            "Checking the default branch for project \"{}\"...",
            project.name
        );
        // Mint a HandlerStatusOp whose opaque id correlates the busy to its final.
        // The resolver captures the project name and re-emits the byte-identical
        // message for every terminal outcome of the two-worker chain (resolved in
        // `process_worker_event` for the short-circuit cases and the switch result).
        let project_name = project.name.clone();
        let op = crate::engine::status_op(busy).resolve_in_handler(
            move |o: &crate::engine::WebCheckoutOutcome| {
                use crate::engine::{Final, WebCheckoutOutcome};
                match o {
                    WebCheckoutOutcome::Ok { target_branch } => Final::info(format!(
                        "Checked out \"{target_branch}\" for project \"{project_name}\"."
                    )),
                    // STICKY: the checkout stopped part-way and the repository
                    // is left on whatever branch it landed on. The message says
                    // outright that the fix is in the user's terminal, which is
                    // as far outside the toast as recovery gets.
                    WebCheckoutOutcome::Failed {
                        target_branch,
                        repo_path,
                    } => Final::error(format!(
                        "Couldn't check out \"{target_branch}\" in {repo_path} — resolve in your terminal and retry."
                    ))
                    .sticky(),
                    WebCheckoutOutcome::AlreadyLeading { current_branch } => Final::info(format!(
                        "Project \"{project_name}\" is already on the leading branch \"{current_branch}\"."
                    )),
                    // STICKY: same shape, same instruction. dux cannot proceed
                    // and is asking the user to go and settle the repository's
                    // default branch by hand before retrying.
                    WebCheckoutOutcome::Heuristic { current_branch } => Final::error(format!(
                        "Can't determine the default branch for project \"{project_name}\" while it is on \"{current_branch}\". Resolve the default branch in your terminal and retry."
                    ))
                    .sticky(),
                    WebCheckoutOutcome::InspectFailed { error } => Final::error(format!(
                        "Couldn't inspect the default branch for project \"{project_name}\": {error}"
                    )),
                }
            },
        );
        let op = op.with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = WireStatus::from_update(&self.begin_status_op(&op));
        self.pending_web_checkout_ops.insert(op_id.clone(), op);
        let worker_tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Guarantee the op resolves even if the job panics: the busy is
            // already on screen, and a panic here would otherwise leave a
            // spinner nothing will ever answer.
            let tx_panic = worker_tx.clone();
            let project_panic = project.clone();
            let op_id_panic = op_id.clone();
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::project_browser::run_checkout_project_default_branch_inspection_job(
                    project,
                    worker_tx,
                    Some(op_id),
                );
            }))
            .is_err()
            {
                let _ = tx_panic.send(
                    crate::worker::WorkerEvent::CheckoutProjectDefaultBranchInspected {
                        project: project_panic,
                        result: Err("the default-branch inspection worker panicked".to_string()),
                        status_op_id: Some(op_id_panic),
                    },
                );
            }
        });
        Ok(pending)
    }

    /// Check out the repo's default branch first, then add it as a project,
    /// mirroring the TUI's "Check Out & Add" path
    /// (`dispatch_non_default_branch_checkout` with a `NonDefaultBranchAction::AddProject`).
    ///
    /// Re-validates server-side rather than trusting the client: the path must be
    /// a git repo whose `branch_warning_kind` is `Known` (a confidently-resolved
    /// default branch that differs from the current one). The heuristic case
    /// never offers this option in the TUI, and an already-on-default repo has no
    /// warning at all, so both are rejected here.
    ///
    /// `git switch` rewrites the working tree, so the actual work runs in
    /// `run_add_project_checkout_job` off-thread (L4 worker chain); this returns
    /// the busy status synchronously, byte-identical to the TUI's `set_busy`.
    fn add_project_checkout_default(
        &mut self,
        path: &str,
        name: String,
    ) -> anyhow::Result<WireStatus> {
        let validated = self
            .validate_project_add_path(path)
            .map_err(|e| anyhow::anyhow!(e))?;
        // A confirmed unborn repo has no default branch to check out; reject
        // explicitly rather than relying on `branch_warning_kind` to bail with a
        // less specific message. Fail open on an indeterminate git result.
        if crate::git::repo_commit_state(&validated) == crate::git::CommitState::Unborn {
            anyhow::bail!(
                "\"{}\" has no commits yet, so there's no default branch to check out. Create an initial commit first.",
                validated.display()
            );
        }
        let branch = crate::git::current_branch_opt(&validated)?;
        let default_branch = match branch.as_deref() {
            // On a normal HEAD: require a Known default (Heuristic is rejected).
            Some(current) => match crate::git::branch_warning_kind(&validated, current) {
                Some(crate::worker::BranchWarningKind::Known { default_branch }) => default_branch,
                _ => anyhow::bail!(
                    "Cannot determine a default branch to check out for \"{}\". Switch branches in your terminal and retry.",
                    validated.display()
                ),
            },
            // On a detached HEAD: try origin/HEAD directly before giving up.
            None => match crate::git::remote_default_branch(&validated) {
                Some(default) => default,
                None => anyhow::bail!(
                    "HEAD is detached and no remote default branch (origin/HEAD) is \
                         configured; check out a branch first."
                ),
            },
        };
        let leading_branch =
            crate::project_browser::leading_branch_for_project(&validated, branch.as_deref());
        let path_str = validated.to_string_lossy().to_string();
        let action = NonDefaultBranchAction::AddProject {
            path: path_str.clone(),
            name,
            leading_branch,
        };
        // Mirror the TUI's `dispatch_non_default_branch_checkout` busy copy
        // (reason "before adding the project").
        let busy =
            format!("Checking out \"{default_branch}\" in {path_str} before adding the project...");
        // Mint a HandlerStatusOp: the SUCCESS final is resolved in
        // `drive_add_project_followup` (after the inline add yields its combined
        // message) and the switch FAILURE in `process_worker_event`; both share
        // this op's opaque id so the busy is replaced, not stranded.
        let op = crate::engine::status_op(busy).resolve_in_handler(
            move |o: &crate::engine::WebAddProjectOutcome| {
                use crate::engine::{Final, WebAddProjectOutcome};
                match o {
                    WebAddProjectOutcome::Added { status_message } => {
                        Final::info(status_message.clone())
                    }
                    // STICKY: the project was added but the branch switch it
                    // was supposed to make did not happen, so the checkout is
                    // not where the user asked for it and the message sends
                    // them to their terminal to finish the job.
                    WebAddProjectOutcome::SwitchFailed {
                        target_branch,
                        repo_path,
                    } => Final::error(format!(
                        "Couldn't check out \"{target_branch}\" in {repo_path} — resolve in your terminal and retry."
                    ))
                    .sticky(),
                    WebAddProjectOutcome::AddFailed { message } => Final::error(message.clone()),
                }
            },
        );
        let op = op.with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = WireStatus::from_update(&self.begin_status_op(&op));
        self.pending_web_add_project_ops.insert(op_id.clone(), op);
        let worker_tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Guarantee the op resolves even if the job panics; see the sibling
            // initial-commit spawn for the same reasoning.
            let tx_panic = worker_tx.clone();
            let action_panic = action.clone();
            let branch_panic = default_branch.clone();
            let op_id_panic = op_id.clone();
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::project_browser::run_add_project_checkout_job(
                    action,
                    default_branch,
                    worker_tx,
                    Some(op_id),
                );
            }))
            .is_err()
            {
                let _ = tx_panic.send(
                    crate::worker::WorkerEvent::NonDefaultBranchCheckoutCompleted {
                        action: action_panic,
                        target_branch: branch_panic,
                        result: Err("the branch-checkout worker panicked".to_string()),
                        status_op_id: Some(op_id_panic),
                    },
                );
            }
        });
        Ok(pending)
    }

    /// Create an empty initial commit for an unborn repo, then register it.
    /// Borrows the worker-chain SHAPE of [`Self::add_project_checkout_default`]
    /// (validate → busy status → worker → followup), but ADDITIONALLY serializes
    /// per repo path via `InFlightKey::InitialCommit` — a gate the checkout-add
    /// flow doesn't need, since a `git switch` can't silently double-append the
    /// way an empty-commit bootstrap could. Worker completion drives
    /// `AddProjectAfterInitialCommit`.
    fn add_project_create_initial_commit(
        &mut self,
        path: &str,
        name: String,
    ) -> anyhow::Result<Option<WireStatus>> {
        let validated = self
            .validate_project_add_path(path)
            .map_err(|e| anyhow::anyhow!(e))?;
        let path_str = validated.to_string_lossy().to_string();
        // Handle the race where a commit landed between the client's inspect and
        // this request. Fail-closed commit state: only Unborn proceeds to the
        // bootstrap; a now-Born repo is just registered (no commit needed), and an
        // indeterminate git state is a hard, synchronous error (not a stranded
        // busy that expires to a 202).
        match crate::git::repo_commit_state(&validated) {
            crate::git::CommitState::Unborn => {}
            crate::git::CommitState::Born => {
                // Already has commits — nothing to bootstrap. Register it directly
                // via the normal add path so the client's request still succeeds
                // (a true no-op on the bootstrap, not a hard failure). Map the
                // reaction to a status EXACTLY as the plain `AddProject` handler
                // does: only a confirmed `Added` outcome is "info"; a rolled-back
                // add relays its error tone; and a deferred (reload-barrier)
                // `Nothing` yields `None` (no toast) rather than a fabricated
                // success.
                let cmd = self.wire_to_command(WireCommand::AddProject {
                    path: path_str,
                    name,
                })?;
                let reaction = self.apply(cmd)?;
                let status = match added_status_message(&reaction) {
                    Some(message) => Some(WireStatus::new("info", message)),
                    None => wire_status_from_reaction(&reaction),
                };
                return Ok(status);
            }
            crate::git::CommitState::Indeterminate => {
                anyhow::bail!(
                    "Couldn't determine the commit state of \"{path_str}\"; not creating an initial commit. Check the repository and retry."
                );
            }
        }
        // Real branch the commit will land on (propagate a git error rather than
        // silently defaulting the branch, matching the sibling add paths).
        let branch = crate::git::current_branch_opt(&validated)?.unwrap_or_default();
        let leading_branch = crate::project_browser::leading_branch_for_project(
            &validated,
            (!branch.is_empty()).then_some(branch.as_str()),
        );
        // Serialize per repo path: reject a second concurrent request rather than
        // let two workers both bootstrap. Cleared on completion in
        // `process_worker_event` (and on a worker panic, below).
        if !self.mark_in_flight(crate::engine::InFlightKey::InitialCommit(path_str.clone())) {
            anyhow::bail!(
                "An initial commit is already being created for \"{path_str}\". Please wait for it to finish."
            );
        }
        let add = crate::worker::InitialCommitAdd {
            path: path_str.clone(),
            name,
            branch,
            leading_branch,
            initialized_repo: false,
            seeded_gitignore: false,
            seed_warning: None,
        };
        let busy = format!("Creating an initial commit in {path_str} before adding the project...");
        // Mint a HandlerStatusOp: SUCCESS resolved in `drive_add_project_followup`
        // after the inline add; FAILURE resolved in `process_worker_event`. Both
        // share this op id so the busy is replaced, not stranded.
        let op = crate::engine::status_op(busy).resolve_in_handler(
            move |o: &crate::engine::WebAddProjectOutcome| {
                use crate::engine::{Final, WebAddProjectOutcome};
                match o {
                    WebAddProjectOutcome::Added { status_message } => {
                        Final::info(status_message.clone())
                    }
                    WebAddProjectOutcome::AddFailed { message } => Final::error(message.clone()),
                    // This flow never switches branches, so `SwitchFailed` can't be
                    // produced for it — but the op type is shared with the
                    // checkout-add flow, so the match must stay total.
                    WebAddProjectOutcome::SwitchFailed { repo_path, .. } => Final::error(format!(
                        "Couldn't add the project at {repo_path} after creating the initial commit."
                    )),
                }
            },
        );
        let op = op.with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = WireStatus::from_update(&self.begin_status_op(&op));
        self.pending_web_add_project_ops.insert(op_id.clone(), op);
        let worker_tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Guarantee the in-flight gate clears and the op resolves even if the
            // job panics — otherwise the gate would strand the repo path forever.
            let tx_panic = worker_tx.clone();
            let add_panic = add.clone();
            let op_id_panic = op_id.clone();
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::project_browser::run_create_initial_commit_job(add, worker_tx, Some(op_id));
            }))
            .is_err()
            {
                let _ = tx_panic.send(crate::worker::WorkerEvent::InitialCommitCreated {
                    add: add_panic,
                    result: Err("the initial-commit worker panicked".to_string()),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok(Some(pending))
    }

    /// Initialize a plain folder as a brand-new git repository, seed a starter
    /// `.gitignore`, create the empty initial commit, then register it. A
    /// near-copy of [`Self::add_project_create_initial_commit`] with these
    /// deltas: validation goes through `validate_project_init_path` (fail-closed,
    /// it front-runs `git init`), there is no Born fast-path (the worker's
    /// re-classification owns all races), and the worker is `run_init_repo_job`.
    /// It reuses `InFlightKey::InitialCommit` so init-and-commit and commit-only
    /// on the same path are mutually exclusive for free.
    fn add_project_init_repo(&mut self, path: &str, name: String) -> anyhow::Result<WireStatus> {
        let validated = self
            .validate_project_init_path(path)
            .map_err(|e| anyhow::anyhow!(e))?;
        let path_str = validated.to_string_lossy().to_string();
        // Serialize per path: reject a second concurrent bootstrap (init or
        // commit-only) rather than let two workers race. Cleared on completion
        // in `process_worker_event` (and on a worker panic, below).
        if !self.mark_in_flight(crate::engine::InFlightKey::InitialCommit(path_str.clone())) {
            anyhow::bail!(
                "A repository is already being initialized in \"{path_str}\". Please wait for it to finish."
            );
        }
        let add = crate::worker::InitialCommitAdd {
            path: path_str.clone(),
            name,
            // The worker resolves the real branch after the commit lands; these
            // placeholders are rewritten by `init_repo_and_commit`.
            branch: String::new(),
            leading_branch: String::new(),
            initialized_repo: false,
            seeded_gitignore: false,
            seed_warning: None,
        };
        let busy =
            format!("Initializing a git repository in {path_str} before adding the project...");
        let op = crate::engine::status_op(busy).resolve_in_handler(
            move |o: &crate::engine::WebAddProjectOutcome| {
                use crate::engine::{Final, WebAddProjectOutcome};
                match o {
                    WebAddProjectOutcome::Added { status_message } => {
                        Final::info(status_message.clone())
                    }
                    WebAddProjectOutcome::AddFailed { message } => Final::error(message.clone()),
                    // This flow never switches branches; the op type is shared
                    // with the checkout-add flow, so the match must stay total.
                    WebAddProjectOutcome::SwitchFailed { repo_path, .. } => Final::error(format!(
                        "Couldn't add the project at {repo_path} after initializing the repository."
                    )),
                }
            },
        );
        let op = op.with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = WireStatus::from_update(&self.begin_status_op(&op));
        self.pending_web_add_project_ops.insert(op_id.clone(), op);
        let worker_tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Guarantee the in-flight gate clears and the op resolves even if
            // the job panics.
            let tx_panic = worker_tx.clone();
            let add_panic = add.clone();
            let op_id_panic = op_id.clone();
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::project_browser::run_init_repo_job(add, worker_tx, Some(op_id));
            }))
            .is_err()
            {
                let _ = tx_panic.send(crate::worker::WorkerEvent::InitialCommitCreated {
                    add: add_panic,
                    result: Err("the repository-initialization worker panicked".to_string()),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok(pending)
    }

    /// Resolve a GitHub PR and create an agent on its head branch, mirroring the
    /// TUI's `open_new_agent_from_pr_prompt` + `dispatch_pull_request_lookup`.
    ///
    /// The TUI does this in two steps: a `gh pr view` lookup worker, then a name
    /// prompt before dispatching the create. The web sends the name UPFRONT, so
    /// this validates the synchronous guards here, carries the name through the
    /// SAME shared lookup worker (`gh::run_pull_request_lookup_job`), and returns
    /// a busy status. On resolution the worker posts `PullRequestResolved`, whose
    /// `OpenNewAgentPromptForPr` reaction the actor's `drive_pr_lookup_followup`
    /// turns into the actual `CreateAgentRequest::PullRequest` dispatch — where
    /// the TUI would open its name prompt instead.
    ///
    /// `git fetch`/`worktree add` (the write) happen later in the create worker,
    /// so nothing here touches the working tree; the only inline work is cheap
    /// validation, matching the CLAUDE.md workers tenet.
    fn create_agent_from_pr(
        &mut self,
        project_id: &str,
        pr: &str,
        name: String,
    ) -> anyhow::Result<WireStatus> {
        // Mirror the TUI's `open_new_agent_from_pr_prompt` gating: the PR flow is
        // unavailable unless GitHub integration is on AND `gh` is installed and
        // authenticated. The web dialog already hides the mode via the ViewModel's
        // `gh_available`, but a raw/stale client must still be rejected.
        if !self.pr_agent_command_available() {
            anyhow::bail!(
                "GitHub PR agent creation requires GitHub integration and an authenticated gh CLI."
            );
        }
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
        // Mirror the TUI's path-missing guard before dispatching the lookup.
        if project.path_missing {
            anyhow::bail!(
                "Cannot create an agent from a PR: path not found for \"{}\"",
                project.name
            );
        }

        // Parse the PR reference up front so a garbage input fails synchronously
        // with an actionable message (the TUI's lookup worker would otherwise
        // surface it asynchronously, but the web has the project remote available
        // here only via the worker — so we validate the NAME synchronously and let
        // the shared worker re-parse the PR against the live remote, which is the
        // single source of host/owner_repo truth). An empty `pr` is caught here.
        if pr.trim().is_empty() {
            anyhow::bail!("Enter a GitHub PR URL or PR number.");
        }

        // Validate the name with the SAME backstop as `CreateAgent`/`ForkSession`:
        // a non-empty name must be a valid agent/branch ref. An empty name is
        // allowed and falls back to the PR head branch (the TUI prompt seeds the
        // head branch as its default), so it travels as `None`.
        let trimmed = name.trim();
        let custom_name = if trimmed.is_empty() {
            None
        } else {
            if !crate::git::is_valid_agent_name(trimmed) {
                anyhow::bail!(
                    "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                     underscores and slashes; it must start with a letter or digit, must \
                     not contain \"//\", and must not end with \"/\"."
                );
            }
            Some(trimmed.to_string())
        };

        // Spawn the shared lookup worker (the TUI's `dispatch_pull_request_lookup`
        // does the same with `None` for the name). Busy copy mirrors the TUI's
        // `set_busy` in `dispatch_pull_request_lookup`.
        let busy = format!("Resolving PR for project \"{}\"...", project.name);
        // Mint a HandlerStatusOp: on SUCCESS the lookup hands off to the create
        // dispatch (whose busy, keyed by the shared create op's opaque id, takes
        // over), so this op's busy is cleared with no message (resolved in
        // `drive_pr_lookup_followup`); on FAILURE it resolves to the keyed error
        // (in `process_worker_event`).
        let op = crate::engine::status_op(busy).resolve_in_handler(
            move |o: &crate::engine::WebPrLookupOutcome| {
                use crate::engine::{Final, WebPrLookupOutcome};
                match o {
                    WebPrLookupOutcome::HandedOff => Final::clear(),
                    WebPrLookupOutcome::Failed { message } => Final::error(message.clone()),
                }
            },
        );
        let op = op.with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = WireStatus::from_update(&self.begin_status_op(&op));
        self.pending_web_pr_lookup_ops.insert(op_id.clone(), op);
        let raw_input = pr.to_string();
        let worker_tx = self.worker_tx.clone();
        let policy = self.github_host_policy();
        std::thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Guarantee the op resolves even if the job panics; see the sibling
            // initial-commit spawn for the same reasoning.
            let tx_panic = worker_tx.clone();
            let op_id_panic = op_id.clone();
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::gh::run_pull_request_lookup_job(
                    project,
                    raw_input,
                    custom_name,
                    worker_tx,
                    Some(op_id),
                    policy,
                );
            }))
            .is_err()
            {
                let _ = tx_panic.send(crate::worker::WorkerEvent::PullRequestResolved {
                    result: Err("the pull-request lookup worker panicked".to_string()),
                    purpose: crate::worker::PrLookupPurpose::CreateAgent,
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok(pending)
    }

    /// Drive a PR-lookup follow-up to completion, returning user-facing statuses.
    /// Called from the web engine actor's worker-event drain alongside the other
    /// `drive_*_followup`s: when `gh::run_pull_request_lookup_job` resolves a PR,
    /// `process_worker_event` produces `OpenNewAgentPromptForPr` (the TUI opens a
    /// name prompt for that reaction). The web already has the name (carried
    /// through the lookup as `ResolvedPullRequest::custom_name`), so this builds
    /// the `CreateAgentRequest::PullRequest` and dispatches the create directly,
    /// mirroring the TUI's `OpenNewAgentPromptForPr` arm but without the prompt.
    ///
    /// `use_existing_branch` is `false`, exactly as the TUI's PR path sets it: the
    /// create worker (`agent_job.rs`'s `PullRequest` arm) does its own last-mile
    /// `use_existing_branch || branch_exists(...)` check, so a head branch that
    /// already exists locally is attached rather than re-fetched without any
    /// pre-computation here. A lookup FAILURE instead produced an error `Status`,
    /// surfaced by the actor's `wire_statuses_from_reaction` drain. Other
    /// reactions return `[]`.
    pub fn drive_pr_lookup_followup(&mut self, reaction: &EventReaction) -> WebFollowupStatuses {
        match reaction {
            EventReaction::OpenNewAgentPromptForPr { pr, status_op_id } => {
                let pr = pr.as_ref();
                // Seed the head branch as the name when no custom name was sent,
                // matching the TUI prompt's default (`Some(head_ref_name)`).
                // An EXPLICIT name was already validated where the request was
                // built; the fallback was not, and a head branch is the remote's
                // string rather than dux's, so it gets the same validator the
                // TUI applies when the user confirms that seeded prompt. Refuse
                // rather than sanitise: a name dux invented is one the user
                // cannot predict and did not ask for, and it would become a
                // branch and a directory on disk.
                let custom_name = match &pr.custom_name {
                    Some(name) => Some(name.clone()),
                    None if crate::git::is_valid_agent_name(&pr.head_ref_name) => {
                        Some(pr.head_ref_name.clone())
                    }
                    None => {
                        let mut clear_keys = Vec::new();
                        if let Some(id) = status_op_id
                            && let Some(op) = self.pending_web_pr_lookup_ops.remove(id)
                        {
                            let resolved =
                                op.resolve(&crate::engine::WebPrLookupOutcome::HandedOff);
                            if let EventReaction::ClearStatus(key) = resolved.into_reaction() {
                                clear_keys.push(key);
                            }
                        }
                        return WebFollowupStatuses {
                            statuses: vec![WireStatus::new(
                                "error",
                                format!(
                                    "PR #{} has head branch \"{}\", which is not a usable agent \
                                     name. Create the agent again and type a name using only \
                                     letters, digits, dashes, underscores and slashes.",
                                    pr.number, pr.head_ref_name
                                ),
                            )],
                            clear_keys,
                        };
                    }
                };
                let resolved_name = custom_name.clone().unwrap_or_default();
                // Busy copy mirrors the TUI's PR create message (input.rs
                // NameNewAgent confirm, PullRequest arm).
                let busy_message = format!(
                    "Creating a new agent worktree \"{resolved_name}\" from PR #{} for project \"{}\" and launching a fresh session...",
                    pr.number, pr.project.name
                );
                let request = CreateAgentRequest::PullRequest {
                    project: pr.project.clone(),
                    host: pr.host.clone(),
                    owner_repo: pr.owner_repo.clone(),
                    number: pr.number,
                    title: pr.title.clone(),
                    state: pr.state.clone(),
                    head_branch: pr.head_ref_name.clone(),
                    custom_name,
                    use_existing_branch: false,
                };
                // The nested create dispatch mints its own busy from
                // `current_origin`, but by now (a worker-completion tick) it has
                // been reset to `All`. Re-set it to the PR-lookup op's captured
                // scope so the handed-off create busy/final stay scoped to the
                // originating connection, then restore `All` (mirroring ApplyWire).
                let origin = status_op_id
                    .as_ref()
                    .and_then(|id| self.pending_web_pr_lookup_ops.get(id))
                    .map(|op| op.scope().clone())
                    .unwrap_or(crate::statusline::StatusScope::All);
                self.current_origin = origin;
                let statuses = match self.apply(Command::DispatchCreateAgentRequest {
                    request: Box::new(request),
                    busy_message: busy_message.clone(),
                    term_size: (80, 24),
                }) {
                    Ok(reaction) => wire_statuses_from_reaction(&reaction),
                    Err(e) => vec![WireStatus::new(
                        "error",
                        format!("Failed to create an agent from PR #{}: {e:#}", pr.number),
                    )],
                };
                self.current_origin = crate::statusline::StatusScope::All;
                // The lookup busy hands off to the create dispatch's busy (emitted
                // above, keyed by the shared create op's opaque id), so resolve the
                // PR-lookup op to a CLEAR so the `Resolving PR…` spinner is
                // dismissed rather than stranded.
                // Done even when the create dispatch Errs (no create busy opened),
                // so the spinner never survives to the timeout.
                let mut clear_keys = Vec::new();
                if let Some(id) = status_op_id
                    && let Some(op) = self.pending_web_pr_lookup_ops.remove(id)
                {
                    let resolved = op.resolve(&crate::engine::WebPrLookupOutcome::HandedOff);
                    if let EventReaction::ClearStatus(key) = resolved.into_reaction() {
                        clear_keys.push(key);
                    }
                }
                WebFollowupStatuses {
                    statuses,
                    clear_keys,
                }
            }
            _ => WebFollowupStatuses::default(),
        }
    }

    /// Drive an add-project follow-up to completion, returning user-facing
    /// statuses. Called from the web engine actor's worker-event drain alongside
    /// `drive_checkout_followup`: when worker 2's `git switch` for an
    /// `AddProjectCheckoutDefault` completes successfully,
    /// `process_worker_event` produces `AddProjectAfterBranchCheckout` (the TUI
    /// drives the same reaction from `workers.rs`). This applies the project-add
    /// INLINE (synchronous engine call: SQLite + config.toml write through the
    /// eager queue, with SQLite rollback on failure), so the new project is in the
    /// engine's in-memory list by the time this returns and appears in the same
    /// ViewModel push. On success it returns the combined "Checked out X and added
    /// project Y" status, mirroring the TUI's `finish_add_project_with_status`
    /// message; on a rolled-back add it relays the engine's error `Status`. A
    /// switch FAILURE (before this runs) instead produces an error `Status`
    /// reaction, surfaced by the actor's `wire_statuses_from_reaction` drain.
    /// Other reactions return `[]`.
    pub fn drive_add_project_followup(&mut self, reaction: &EventReaction) -> Vec<WireStatus> {
        match reaction {
            EventReaction::AddProjectAfterBranchCheckout {
                path,
                name,
                target_branch,
                leading_branch,
                status_op_id,
            } => {
                let display_name = display_project_name(name, path);
                let status_message = format!(
                    "Checked out \"{target_branch}\" and added project \"{display_name}\" to the workspace."
                );
                let add_failed_prefix =
                    format!("Checked out \"{target_branch}\" but couldn't add the project");
                self.finish_web_project_add(WebProjectAdd {
                    path,
                    display_name,
                    current_branch: target_branch,
                    leading_branch,
                    status_message,
                    add_failed_prefix: &add_failed_prefix,
                    status_op_id,
                })
            }
            EventReaction::AddProjectAfterInitialCommit {
                path,
                name,
                branch,
                leading_branch,
                initialized_repo,
                seeded_gitignore,
                seed_warning,
                status_op_id,
            } => {
                let display_name = display_project_name(name, path);
                let status_message = if *initialized_repo && *seeded_gitignore {
                    format!(
                        "Initialized a git repository, seeded a starter .gitignore, created an initial commit, and added project \"{display_name}\" to the workspace."
                    )
                } else if *initialized_repo {
                    format!(
                        "Initialized a git repository, created an initial commit, and added project \"{display_name}\" to the workspace."
                    )
                } else {
                    format!(
                        "Created an initial commit and added project \"{display_name}\" to the workspace."
                    )
                };
                let add_failed_prefix = if *initialized_repo {
                    "Initialized the repository but couldn't add the project"
                } else {
                    "Created the initial commit but couldn't add the project"
                };
                let mut statuses = self.finish_web_project_add(WebProjectAdd {
                    path,
                    display_name,
                    current_branch: branch,
                    leading_branch,
                    status_message,
                    add_failed_prefix,
                    status_op_id,
                });
                // A non-fatal seed failure rides alongside the final as its own
                // persistent warning toast so the user can still act on it.
                if let Some(warning) = seed_warning {
                    statuses.push(WireStatus::new("warning", warning.clone()));
                }
                statuses
            }
            _ => vec![],
        }
    }

    /// Shared tail of the "do git work, then register the project" web
    /// followups: build the `Project`, apply the INLINE add (config write with
    /// SQLite rollback), and resolve the keyed add-project op. Used by both the
    /// checkout-default and initial-commit flows so the delicate status-op
    /// correlation lives in one place. Args are a named-field struct so the two
    /// adjacent branch strings can't be silently transposed.
    fn finish_web_project_add(&mut self, args: WebProjectAdd) -> Vec<WireStatus> {
        let WebProjectAdd {
            path,
            display_name,
            current_branch,
            leading_branch,
            status_message,
            add_failed_prefix,
            status_op_id,
        } = args;
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: display_name,
            path: path.to_string(),
            explicit_default_provider: None,
            default_provider: self.config.default_provider(),
            leading_branch: Some(leading_branch.to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: std::collections::BTreeMap::new(),
            current_branch: current_branch.to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: Some(chrono::Utc::now()),
        };
        // The add is INLINE: the handler writes config.toml (with SQLite
        // rollback on failure) and returns the real outcome NOW. On
        // success it returns `ProjectPersistenceOutcome(Added)`; on a
        // config-write/DB failure it returns an error-toned `Status`
        // (still a Rust `Ok`, but the add was rolled back). Inspect the
        // reaction so a rolled-back add is reported as the failure it was,
        // not the optimistic "added project" success.
        //
        // The user-facing statuses (`statuses`) stay byte-identical to the
        // pre-StatusOp behavior. When `status_op_id` is Some (always, for
        // the web), we ALSO resolve the add-project op so its busy is
        // replaced by the keyed final instead of being separately cleared.
        // The inline persist can mint a status from `current_origin`;
        // re-set it to the add-project op's captured scope (reset to `All`
        // by the worker tick) so any minted status stays scoped to the
        // originating connection, then restore `All` afterward.
        let origin = status_op_id
            .as_ref()
            .and_then(|id| self.pending_web_add_project_ops.get(id))
            .map(|op| op.scope().clone())
            .unwrap_or(crate::statusline::StatusScope::All);
        self.current_origin = origin;
        // The engine's Added outcome carries the AUTHORITATIVE message — which is
        // the dedup chokepoint's honest "already in the workspace" text when a
        // race hit that path, not this caller's optimistic narrative. Surface
        // that (falling back to the caller's message only if the engine didn't
        // provide one), for both the unkeyed status and the keyed op below.
        let mut success_message = status_message.clone();
        let statuses = match self.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Add {
                project,
                status_message: status_message.clone(),
            }),
            status_op_id: None,
        }) {
            Ok(EventReaction::ProjectPersistenceOutcome(outcome))
                if matches!(outcome.view, ProjectPersistenceView::Added { .. }) =>
            {
                if let ProjectPersistenceView::Added {
                    status_message: engine_message,
                    ..
                } = &outcome.view
                {
                    success_message = engine_message.clone();
                }
                vec![WireStatus::new("info", success_message.clone())]
            }
            // A rolled-back add surfaces as an error-toned Status; relay it
            // verbatim so the user learns the add failed and was undone.
            Ok(reaction) => wire_statuses_from_reaction(&reaction),
            Err(e) => vec![WireStatus::new(
                "error",
                format!("{add_failed_prefix}: {e:#}"),
            )],
        };
        self.current_origin = crate::statusline::StatusScope::All;

        // Resolve the add-project op against the same outcome the
        // `statuses` carry, keying the final to the op's id so it replaces
        // the busy. The op's resolver re-emits the SAME message: a clean
        // add → `Added` (info), any failure → `AddFailed` (the relayed
        // error text). When no op is registered (id None, or already
        // consumed by a switch-failure path), fall back to `statuses`.
        if let Some(id) = status_op_id
            && let Some(op) = self.pending_web_add_project_ops.remove(id)
        {
            let is_success = statuses.iter().any(|s| s.tone == "info");
            let outcome = if is_success {
                crate::engine::WebAddProjectOutcome::Added {
                    status_message: success_message.clone(),
                }
            } else {
                // Surface the same failure text the unkeyed `statuses`
                // would have shown (the engine's rolled-back error or the
                // apply error), now keyed so it replaces the busy.
                let message = statuses
                    .iter()
                    .find(|s| s.tone == "error")
                    .map(|s| s.message.clone())
                    .unwrap_or_else(|| status_message.clone());
                crate::engine::WebAddProjectOutcome::AddFailed { message }
            };
            // The add-project op always resolves to a Message (never a
            // Clear), so `into_reaction()` is a keyed `Status` that
            // `wire_statuses_from_reaction` renders directly.
            return wire_statuses_from_reaction(&op.resolve(&outcome).into_reaction());
        }
        statuses
    }

    /// Drive a checkout-related reaction to completion, returning user-facing
    /// statuses. Called from the web engine actor's worker-event drain alongside
    /// `drive_delete_followup`: when worker 1's inspection produces a
    /// `DispatchProjectDefaultBranchCheckout` (the Known default-branch case),
    /// spawn worker 2 (`run_add_project_checkout_job`) to run `git switch`
    /// off-thread, mirroring the TUI's `dispatch_non_default_branch_checkout`.
    /// Worker 2's completion posts `NonDefaultBranchCheckoutCompleted`, whose
    /// `process_worker_event` arm returns the success/failure `Status` that the
    /// actor's existing `wire_statuses_from_reaction` drain broadcasts. Other
    /// reactions return `[]`.
    pub fn drive_checkout_followup(&mut self, reaction: &EventReaction) -> Vec<WireStatus> {
        match reaction {
            EventReaction::DispatchProjectDefaultBranchCheckout {
                project,
                default_branch,
                status_op_id,
            } => {
                let action = NonDefaultBranchAction::CheckoutProjectDefault {
                    project: project.clone(),
                };
                let target_branch = default_branch.clone();
                // Forward the checkout op's id into worker 2 so its eventual
                // `NonDefaultBranchCheckoutCompleted` resolves the right op.
                let status_op_id = status_op_id.clone();
                let worker_tx = self.worker_tx.clone();
                std::thread::spawn(move || {
                    crate::project_browser::run_add_project_checkout_job(
                        action,
                        target_branch,
                        worker_tx,
                        status_op_id,
                    );
                });
                vec![]
            }
            _ => vec![],
        }
    }

    /// Drive a delete-related reaction to completion, returning user-facing
    /// statuses. Used by `apply_wire` (synchronous Begin/Inline) and by the web
    /// engine actor's worker-event drain (async worktree-removal completion), so
    /// deletions finish without a view layer. Non-delete reactions return `[]`.
    pub fn drive_delete_followup(&mut self, reaction: &EventReaction) -> Vec<WireStatus> {
        match reaction {
            EventReaction::BeginDeleteSessionView(view) => {
                match &view.outcome {
                    BeginDeleteSessionOutcome::AlreadyInFlight => vec![WireStatus::new(
                        "error",
                        "Deletion already in progress for this agent. Wait for it to finish.",
                    )],
                    BeginDeleteSessionOutcome::TabLaunching => vec![WireStatus::new(
                        "error",
                        "A tab is still launching for this agent. Try again in a moment.",
                    )],
                    BeginDeleteSessionOutcome::NotFound => vec![],
                    // The delete did not happen: the caller asked dux to remove
                    // a standalone agent's folder, which it never does. An
                    // error, not an info, because the request was refused
                    // rather than quietly reinterpreted.
                    BeginDeleteSessionOutcome::Refused { message } => {
                        vec![WireStatus::new("error", message.clone())]
                    }
                    BeginDeleteSessionOutcome::AsyncStarted { busy_message } => {
                        // Mint a keyed HandlerStatusOp whose opaque id correlates
                        // this busy to the final resolved when the git-removal
                        // worker reports back, and stash it keyed by session id.
                        // The resolver reproduces the web's exact wording for every
                        // terminal `WebDeleteOutcome`.
                        let op = crate::engine::status_op(busy_message.clone()).resolve_in_handler(
                            |o: &crate::engine::WebDeleteOutcome| {
                                use crate::engine::{Final, WebDeleteOutcome};
                                match o {
                                    WebDeleteOutcome::Succeeded { message } => {
                                        Final::info(message.clone())
                                    }
                                    WebDeleteOutcome::SucceededGone => {
                                        Final::info("Agent and worktree removed.")
                                    }
                                    // STICKY: the agent row is already gone from
                                    // the UI but its worktree is still on disk, so
                                    // the user is left with an orphaned directory
                                    // only `git worktree remove` will clear.
                                    WebDeleteOutcome::Failed { message } => {
                                        Final::error(format!("Worktree delete failed: {message}"))
                                            .sticky()
                                    }
                                    // STICKY: same half-done delete from the other
                                    // side, the worktree went but the records did
                                    // not, so the agent can come back on restart.
                                    WebDeleteOutcome::CleanupFailed { message } => {
                                        Final::error(format!("Session cleanup failed: {message}"))
                                            .sticky()
                                    }
                                }
                            },
                        );
                        let op = op.with_scope(self.current_origin.clone());
                        let pending = WireStatus::from_update(&self.begin_status_op(&op));
                        self.pending_delete_ops_web
                            .insert(view.session_id.clone(), op);
                        // Vanish the session now: its PTY and terminals are
                        // already SIGTERMed and held for a background reap.
                        //
                        // The engine method, not `Command::FinishDeleteSession`,
                        // because this path has no removal to report. The
                        // command carries a `WorktreeRemoval` describing what
                        // happened to the branches, and here nothing has
                        // happened to them yet: the real report arrives with
                        // the deferred removal, which authors the final
                        // message. Going through the command meant
                        // manufacturing a `Deleted(RemoveResult::default())`
                        // that says "the branch was deleted, and no branches
                        // were deleted", relying on the whole reaction being
                        // dropped for that never to be read. The command adds
                        // nothing else here (it wraps this exact call into a
                        // view for the surfaces), so the honest thing is to
                        // call it.
                        let _ = self.finish_delete_session(&view.session_id);
                        vec![pending]
                    }
                    BeginDeleteSessionOutcome::Inline { removal } => {
                        let removal = removal.clone();
                        self.finish_delete_and_status(&view.session_id, removal)
                    }
                }
            }
            EventReaction::WorktreeRemoveSucceeded {
                session_id,
                branches,
                ..
            } => {
                if self.sessions.iter().any(|s| s.id == *session_id) {
                    self.finish_delete_and_status(
                        session_id,
                        WorktreeRemoval::Performed {
                            branches: branches.clone(),
                        },
                    )
                } else {
                    // The session was already removed by another path (e.g. its
                    // project was deleted, taking its sessions with it) before
                    // this worker reported back. The worktree removal still
                    // completed, so resolve the keyed busy with its same-key final
                    // rather than stranding the toast until the busy-timeout
                    // Warning. If no op is stashed (the TUI path, or a synthetic
                    // event), emit nothing.
                    self.resolve_web_delete_op(
                        session_id,
                        &crate::engine::WebDeleteOutcome::SucceededGone,
                    )
                }
            }
            EventReaction::WorktreeRemoveFailed {
                session_id,
                message,
            } => self.resolve_web_delete_op(
                session_id,
                &crate::engine::WebDeleteOutcome::Failed {
                    message: message.clone(),
                },
            ),
            _ => vec![],
        }
    }

    /// Pop the web delete op for `session_id` and resolve it into its keyed final
    /// WireStatus. Returns `[]` when no op is stashed (the TUI path, or a
    /// synthetic completion event with no preceding `AsyncStarted`).
    fn resolve_web_delete_op(
        &mut self,
        session_id: &str,
        outcome: &crate::engine::WebDeleteOutcome,
    ) -> Vec<WireStatus> {
        match self.pending_delete_ops_web.remove(session_id) {
            Some(op) => wire_statuses_from_reaction(&op.resolve(outcome).into_reaction()),
            None => vec![],
        }
    }

    /// Drive the web reconnect / force-restart launch follow-up to completion.
    /// Called from the web engine actor's worker-event drain alongside
    /// `drive_delete_followup`: when a launch reports back, its
    /// `AgentLaunchReadyView` / `AgentLaunchFailedView` reaction resolves the web
    /// launch op (`Engine::pending_web_launch_ops`) stashed by `reconnect_session`,
    /// replacing the "Launching…" / "Starting fresh…" busy with the same-key final.
    ///
    /// This is the web counterpart to the TUI's `resolve_reconnect_op_or`. When no
    /// op is stashed (a resume-fallback retry or a startup auto-reopen, neither of
    /// which goes through `reconnect_session`), the same final is emitted UNKEYED —
    /// byte-identical text, and there is no preceding web busy on those paths to
    /// dismiss. Create-kind launches are resolved engine-side, never here.
    ///
    /// Slot-ness is read from the outcome's SESSION SNAPSHOT here, deliberately,
    /// and this is the one place that must not be swept into asking the live
    /// session. The question being answered is not "which tab is the slot now"
    /// but "which arm requested this launch", and a `Kind::Tab` launch collapses
    /// into the `Reconnect` view, so the snapshot's slot-ness is the only proxy
    /// left for it. A live read would send a tab promoted mid-launch down the
    /// session-keyed branch and strand its `tab-launch-<id>` busy toast forever.
    pub fn drive_web_launch_followup(&mut self, reaction: &EventReaction) -> WebFollowupStatuses {
        match reaction {
            EventReaction::AgentLaunchReadyView(outcome) => match &outcome.view {
                AgentLaunchReadyView::Reconnect { status_message }
                | AgentLaunchReadyView::ResumeFallback { status_message, .. } => {
                    if !self.is_slot_tab(&outcome.session, TabIdRef::new(&outcome.tab_id)) {
                        // Support-tab launch success: no op is stashed under a tab
                        // id, so emit a status KEYED `tab-launch-<tab_id>` (matching
                        // the failure path) rather than an unkeyed one — otherwise an
                        // unrelated unkeyed toast could clobber this confirmation.
                        WebFollowupStatuses {
                            statuses: vec![
                                WireStatus::new("info", status_message.clone())
                                    .with_key(format!("tab-launch-{}", outcome.tab_id)),
                            ],
                            clear_keys: Vec::new(),
                        }
                    } else {
                        // The session-slot tab: resolve its pending reconnect
                        // op, which `reconnect_session` stashed under the
                        // SESSION id, not the tab id.
                        self.resolve_web_launch_op_or(
                            &outcome.session.id,
                            crate::engine::LaunchOutcome::Ready {
                                status_message: status_message.clone(),
                            },
                        )
                    }
                }
                AgentLaunchReadyView::SessionMissing => self.resolve_web_launch_op_or(
                    &outcome.session.id,
                    crate::engine::LaunchOutcome::Missing,
                ),
                // StartupAutoReopen success is silent (mirrors the TUI). A create
                // commit/persist-fail final is resolved engine-side, not here.
                AgentLaunchReadyView::StartupAutoReopen
                | AgentLaunchReadyView::CreateCommitted { .. }
                | AgentLaunchReadyView::CreatePersistFailed { .. } => {
                    WebFollowupStatuses::default()
                }
            },
            EventReaction::AgentLaunchFailedView(outcome) => match outcome.as_ref() {
                AgentLaunchFailedOutcome::Reconnect {
                    session_id,
                    agent_label,
                    message,
                } => self.resolve_web_launch_op_or(
                    session_id,
                    crate::engine::LaunchOutcome::ReconnectFailed {
                        branch_name: agent_label.clone(),
                        message: message.clone(),
                    },
                ),
                AgentLaunchFailedOutcome::ForceReconnect {
                    session_id,
                    agent_label,
                    message,
                } => self.resolve_web_launch_op_or(
                    session_id,
                    crate::engine::LaunchOutcome::ForceReconnectFailed {
                        branch_name: agent_label.clone(),
                        message: message.clone(),
                    },
                ),
                // Startup-auto-reopen failure is an unkeyed warning (no web busy
                // precedes it, as it never goes through `reconnect_session`), and
                // resume-fallback failure is silent — both mirror the TUI.
                AgentLaunchFailedOutcome::StartupAutoReopen {
                    agent_label,
                    message,
                    ..
                } => WebFollowupStatuses {
                    statuses: vec![WireStatus::new(
                        "warning",
                        format!("Couldn't auto-reopen agent \"{agent_label}\": {message}"),
                    )],
                    clear_keys: Vec::new(),
                },
                // Support-tab launch failure: surface the real error as a warning
                // keyed to the tab, so a failed fresh-create / dormant relaunch
                // tells the user why nothing came up (never a "can't resume" note).
                AgentLaunchFailedOutcome::Tab {
                    tab_id,
                    agent_label,
                    message,
                    ..
                } => WebFollowupStatuses {
                    // Keyed by the tab (`tab-launch-<tab_id>`) so this warning is not
                    // clobbered by an unrelated unkeyed toast, and so the web client
                    // can clear its "started" latch for this tab (re-showing the
                    // dormant retry card and stopping the reconnect loop).
                    statuses: vec![
                        WireStatus::new(
                            "warning",
                            format!("Tab launch failed for \"{agent_label}\": {message}"),
                        )
                        .with_key(format!("tab-launch-{tab_id}")),
                    ],
                    clear_keys: Vec::new(),
                },
                // A create-kind launch failure is resolved engine-side, not here.
                // `Silent` is a ghost-tab launch failure: the row is already
                // gone from the user's perspective, so there is nothing to
                // surface, mirroring the ready-path `SessionMissing` view.
                AgentLaunchFailedOutcome::ResumeFallback
                | AgentLaunchFailedOutcome::Create { .. }
                | AgentLaunchFailedOutcome::Silent => WebFollowupStatuses::default(),
            },
            _ => WebFollowupStatuses::default(),
        }
    }

    /// Resolve the web launch op for `session_id` against `outcome`, or apply the
    /// SAME final UNKEYED when no op is stashed (mirroring the TUI's
    /// `resolve_reconnect_op_or` fallback). A `Final::Clear` becomes a `clear_keys`
    /// entry (the op path) or a no-op (the keyless fallback path, where there is no
    /// busy to clear).
    fn resolve_web_launch_op_or(
        &mut self,
        session_id: &str,
        outcome: crate::engine::LaunchOutcome,
    ) -> WebFollowupStatuses {
        match self.pending_web_launch_ops.remove(session_id) {
            Some(op) => match op.resolve(&outcome).into_reaction() {
                EventReaction::Status(update) => WebFollowupStatuses {
                    statuses: vec![WireStatus::from_update(&update)],
                    clear_keys: Vec::new(),
                },
                EventReaction::ClearStatus(key) => WebFollowupStatuses {
                    statuses: Vec::new(),
                    clear_keys: vec![key],
                },
                _ => WebFollowupStatuses::default(),
            },
            None => match crate::engine::launch_outcome_final(&outcome) {
                crate::engine::Final::Message { tone, text, sticky } => WebFollowupStatuses {
                    statuses: vec![{
                        let s = WireStatus::new(tone.as_wire(), text);
                        if sticky { s.sticky() } else { s }
                    }],
                    clear_keys: Vec::new(),
                },
                // No op and no busy to dismiss: nothing to do.
                crate::engine::Final::Clear => WebFollowupStatuses::default(),
            },
        }
    }

    fn finish_delete_and_status(
        &mut self,
        session_id: &str,
        removal: WorktreeRemoval,
    ) -> Vec<WireStatus> {
        // If a keyed op is stashed for this session (the async path emitted its
        // busy), the success message must resolve THAT op so the spinner is
        // replaced by its same-key final. The synchronous inline path has no op
        // stashed, so it falls back to an unkeyed info status (its busy, if any,
        // is the legacy `delete:{id}` key only on the async path — gone now).
        match self.apply(Command::FinishDeleteSession {
            session_id: session_id.to_string(),
            removal,
            update_status: true,
        }) {
            Ok(EventReaction::FinishDeleteSessionView(view)) => {
                let message = delete_session_status_message(&view.outcome, &view.removal);
                match self.pending_delete_ops_web.remove(session_id) {
                    Some(op) => wire_statuses_from_reaction(
                        &op.resolve(&crate::engine::WebDeleteOutcome::Succeeded { message })
                            .into_reaction(),
                    ),
                    None => vec![WireStatus::new("info", message)],
                }
            }
            Ok(_) => vec![],
            Err(e) => match self.pending_delete_ops_web.remove(session_id) {
                Some(op) => wire_statuses_from_reaction(
                    &op.resolve(&crate::engine::WebDeleteOutcome::CleanupFailed {
                        message: format!("{e:#}"),
                    })
                    .into_reaction(),
                ),
                None => vec![WireStatus::new(
                    "error",
                    format!("Session cleanup failed: {e:#}"),
                )],
            },
        }
    }

    fn map_changes_command(&self, command: WireCommand) -> anyhow::Result<WireCommandMapping> {
        let mapped = match command {
            WireCommand::StageFile { session_id, path } => Command::StageFile {
                worktree_path: self.changes_worktree(&session_id)?,
                path,
            },
            WireCommand::UnstageFile { session_id, path } => Command::UnstageFile {
                worktree_path: self.changes_worktree(&session_id)?,
                path,
            },
            WireCommand::DiscardFile { session_id, path } => {
                let worktree_path = self.changes_worktree(&session_id)?;
                let is_untracked = crate::git::discard_classify(&worktree_path, &path)?;
                Command::DiscardFile {
                    worktree_path,
                    path,
                    is_untracked,
                }
            }
            WireCommand::CommitChanges {
                session_id,
                message,
            } => Command::CommitChanges {
                worktree_path: self.changes_worktree(&session_id)?,
                message,
                success_message: "Changes committed successfully.".to_string(),
            },
            WireCommand::Push { session_id } => Command::Push {
                // Push publishes a BRANCH, which is the half that does not
                // exist for a standalone agent. Refused on the server, not
                // merely hidden in the client: this is an HTTP route, and a
                // client that skipped the UI could otherwise reach a real push
                // in the user's folder.
                worktree_path: PathBuf::from(
                    &self
                        .branch_git_workspace(
                            &session_id,
                            "push",
                            crate::engine::STANDALONE_ADD_AS_PROJECT_REMEDY,
                        )?
                        .worktree_path,
                ),
            },
            WireCommand::Pull { session_id } => Command::Pull {
                // Same gate as push, and for the same reason: a pull updates
                // the branch dux manages for this agent, and there is none.
                repo_path: PathBuf::from(
                    &self
                        .branch_git_workspace(
                            &session_id,
                            "pull into",
                            crate::engine::STANDALONE_ADD_AS_PROJECT_REMEDY,
                        )?
                        .worktree_path,
                ),
                target: PullTarget::Session,
                busy_message: "Pulling latest changes from remote\u{2026}".to_string(),
                already_running_message:
                    "Pull already in progress for this worktree. Wait for the current pull to finish."
                        .to_string(),
            },
            command => return Ok(WireCommandMapping::Unhandled(command)),
        };
        Ok(WireCommandMapping::Mapped(mapped))
    }

    fn map_project_command(&self, command: WireCommand) -> anyhow::Result<WireCommandMapping> {
        let mapped = match command {
            WireCommand::PullProject { project_id } => {
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
                if project.path_missing {
                    anyhow::bail!("Cannot refresh: path not found for \"{}\"", project.name);
                }
                Command::Pull {
                    repo_path: PathBuf::from(&project.path),
                    target: PullTarget::Project {
                        project_id: project.id,
                        project_name: project.name.clone(),
                        leading_branch: project.leading_branch.clone(),
                    },
                    busy_message: format!(
                        "Refreshing project \"{}\" from remote\u{2026}",
                        project.name
                    ),
                    already_running_message: format!(
                        "Project refresh already in progress for \"{}\". Wait for the current pull to finish.",
                        project.name,
                    ),
                }
            }
            WireCommand::ToggleAgentAutoReopen {
                session_id,
                enabled,
            } => {
                let branch_name = self
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?
                    .display_label();
                Command::ToggleAgentAutoReopen {
                    session_id,
                    branch_name,
                    new_enabled: enabled,
                }
            }
            WireCommand::UpdateProjectProvider {
                project_id,
                provider,
            } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject {
                    action: Box::new(ProjectPersistenceAction::UpdateDefaultProvider {
                        project_id,
                        project_name,
                        provider: provider.map(ProviderKind::new),
                        global_default: self.config.default_provider(),
                    }),
                    status_op_id: None,
                }
            }
            WireCommand::UpdateProjectAutoReopen {
                project_id,
                auto_reopen_agents,
            } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject {
                    action: Box::new(ProjectPersistenceAction::UpdateAutoReopen {
                        project_id,
                        project_name,
                        auto_reopen_agents,
                    }),
                    status_op_id: None,
                }
            }
            WireCommand::UpdateProjectStartupCommand {
                project_id,
                startup_command,
            } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject {
                    action: Box::new(ProjectPersistenceAction::UpdateStartupCommand {
                        project_id,
                        project_name,
                        startup_command,
                    }),
                    status_op_id: None,
                }
            }
            WireCommand::UpdateProjectEnv { project_id, env } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject {
                    action: Box::new(ProjectPersistenceAction::UpdateEnv {
                        project_id,
                        project_name,
                        env,
                    }),
                    status_op_id: None,
                }
            }
            WireCommand::AddProject { path, name } => {
                let validated = self
                    .validate_project_add_path(&path)
                    .map_err(|e| anyhow::anyhow!(e))?;
                if crate::git::repo_commit_state(&validated) == crate::git::CommitState::Unborn {
                    anyhow::bail!(
                        "\"{}\" has no commits yet, so it can't back a worktree. Create an initial commit first (the app offers to do this for you when adding the project).",
                        validated.display()
                    );
                }
                let branch = crate::git::current_branch_opt(&validated)?;
                let leading_branch = crate::project_browser::leading_branch_for_project(
                    &validated,
                    branch.as_deref(),
                );
                let path_str = validated.to_string_lossy().to_string();
                let display_name = if name.trim().is_empty() {
                    validated
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("project")
                        .to_string()
                } else {
                    name.trim().to_string()
                };
                let project = Project {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: display_name.clone(),
                    path: path_str,
                    explicit_default_provider: None,
                    default_provider: self.config.default_provider(),
                    leading_branch: Some(leading_branch),
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: std::collections::BTreeMap::new(),
                    current_branch: branch.unwrap_or_default(),
                    branch_status: ProjectBranchStatus::Unknown,
                    path_missing: false,
                    created_at: Some(chrono::Utc::now()),
                };
                let status_message = format!("Added project \"{display_name}\" to the workspace.");
                Command::PersistProject {
                    action: Box::new(ProjectPersistenceAction::Add {
                        project,
                        status_message,
                    }),
                    status_op_id: None,
                }
            }
            WireCommand::RemoveProject { project_id } => Command::RemoveProject {
                project_name: self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| crate::sidebar::short_project_id(&project_id)),
                project_id,
            },
            WireCommand::DeleteProject { project_id } => Command::DeleteProject {
                project_name: self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| crate::sidebar::short_project_id(&project_id)),
                project_id,
            },
            command => return Ok(WireCommandMapping::Unhandled(command)),
        };
        Ok(WireCommandMapping::Mapped(mapped))
    }

    fn create_agent_command(
        &self,
        project_id: String,
        name: String,
        copy_uncommitted_changes: Option<bool>,
        use_existing_branch: bool,
    ) -> anyhow::Result<Command> {
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
        let trimmed = name.trim();
        let custom_name = if trimmed.is_empty() {
            None
        } else {
            if !crate::git::is_valid_agent_name(trimmed) {
                anyhow::bail!(
                    "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                     underscores and slashes; it must start with a letter or digit, must \
                     not contain \"//\", and must not end with \"/\"."
                );
            }
            Some(trimmed.to_string())
        };
        if !use_existing_branch
            && let Some(name) = &custom_name
            && matches!(
                crate::git::create_agent_branch_preflight(
                    std::path::Path::new(&project.path),
                    name,
                ),
                crate::git::CreateAgentBranchPlan::ExistingBranch { .. }
            )
        {
            anyhow::bail!(
                "A branch named \"{name}\" already exists. Creating this agent would \
                 attach to that branch's history. Confirm to attach, or pick a different name."
            );
        }
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch,
            pull_before_create: self.config.defaults.pull_before_creating_agent_by_default,
            copy_uncommitted_changes: copy_uncommitted_changes
                .unwrap_or(self.config.defaults.copy_uncommitted_changes_by_default),
        };
        Ok(Command::DispatchCreateAgentRequest {
            request: Box::new(request),
            busy_message: "Creating a new agent\u{2026}".to_string(),
            term_size: (80, 24),
        })
    }

    fn fork_session_command(&self, session_id: String, name: String) -> anyhow::Result<Command> {
        let managed = self
            .branch_git_workspace(
                &session_id,
                "fork",
                crate::engine::STANDALONE_ADD_AS_PROJECT_REMEDY,
            )?
            .clone();
        let source_session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        let project = self
            .projects
            .iter()
            .find(|p| p.id == managed.project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {}", managed.project_id))?;
        let source_label = source_session.display_label();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Agent name cannot be empty.");
        }
        if !crate::git::is_valid_agent_name(trimmed) {
            anyhow::bail!(
                "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                 underscores and slashes; it must start with a letter or digit, must \
                 not contain \"//\", and must not end with \"/\"."
            );
        }
        let name = trimmed.to_string();
        let busy_message = format!(
            "Forking agent \"{source_label}\" as \"{name}\" by cloning its current \
             worktree contents into a fresh session...",
        );
        let request = CreateAgentRequest::ForkSession {
            project,
            source_session: Box::new(source_session),
            source_label,
            custom_name: Some(name),
        };
        Ok(Command::DispatchCreateAgentRequest {
            request: Box::new(request),
            busy_message,
            term_size: (80, 24),
        })
    }

    fn adopt_worktree_command(
        &self,
        project_id: String,
        worktree_path: String,
        name: String,
    ) -> anyhow::Result<Command> {
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
        let requested =
            crate::project_browser::canonical_or_original(std::path::Path::new(&worktree_path));
        let entry = self
            .adoptable_managed_worktrees(&project)?
            .into_iter()
            .find(|entry| entry.path == requested)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Worktree \"{worktree_path}\" is not an adoptable managed worktree for \
                     project \"{}\". Refresh the list and pick a worktree that has no agent yet.",
                    project.name
                )
            })?;
        let trimmed = name.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Agent name cannot be empty.");
        }
        if !crate::git::is_valid_agent_name(trimmed) {
            anyhow::bail!(
                "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                 underscores and slashes; it must start with a letter or digit, must \
                 not contain \"//\", and must not end with \"/\"."
            );
        }
        let display_name = trimmed.to_string();
        let busy_message = format!(
            "Starting agent \"{display_name}\" in existing worktree {} for project \"{}\"...",
            entry.path.display(),
            project.name
        );
        let request = CreateAgentRequest::ExistingManagedWorktree {
            project,
            worktree_path: entry.path.clone(),
            branch_name: entry.branch_name.clone(),
            custom_name: Some(display_name),
        };
        Ok(Command::DispatchCreateAgentRequest {
            request: Box::new(request),
            busy_message,
            term_size: (80, 24),
        })
    }

    fn map_agent_command(&self, command: WireCommand) -> anyhow::Result<WireCommandMapping> {
        let mapped = match command {
            WireCommand::CreateStandaloneAgent {
                folder,
                name,
                provider,
            } => {
                let (request, busy_message) =
                    self.plan_standalone_agent(&folder, &name, provider.as_deref())?;
                Command::DispatchCreateAgentRequest {
                    request: Box::new(request),
                    busy_message,
                    term_size: (80, 24),
                }
            }
            WireCommand::CreateAgent {
                project_id,
                name,
                copy_uncommitted_changes,
                use_existing_branch,
            } => self.create_agent_command(
                project_id,
                name,
                copy_uncommitted_changes,
                use_existing_branch,
            )?,
            WireCommand::ForkSession { session_id, name } => {
                self.fork_session_command(session_id, name)?
            }
            WireCommand::CreateAgentFromWorktree {
                project_id,
                worktree_path,
                name,
            } => self.adopt_worktree_command(project_id, worktree_path, name)?,
            command => return Ok(WireCommandMapping::Unhandled(command)),
        };
        Ok(WireCommandMapping::Mapped(mapped))
    }

    fn update_macros_command(entries: Vec<WireMacroEntry>) -> anyhow::Result<Command> {
        let mut macros = crate::config::MacrosConfig::default();
        for entry in entries {
            let name = entry.name.trim().to_string();
            if name.is_empty() {
                anyhow::bail!("Macro name cannot be empty.");
            }
            if macros.entries.contains_key(&name) {
                anyhow::bail!("Name \"{name}\" is already in use. Choose another.");
            }
            if entry.text.is_empty() {
                anyhow::bail!("Macro \"{name}\" has no text. Enter the text to send.");
            }
            let surface = crate::config::MacroSurface::from_config_str(&entry.surface)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Macro \"{name}\" has an unknown surface \"{}\". Use \"agent\", \"terminal\", or \"both\".",
                        entry.surface
                    )
                })?;
            macros.entries.insert(
                name,
                crate::config::MacroEntry {
                    text: entry.text,
                    surface,
                },
            );
        }
        Ok(Command::UpdateMacros { macros })
    }

    fn wire_to_command(&self, command: WireCommand) -> anyhow::Result<Command> {
        let command = match self.map_changes_command(command)? {
            WireCommandMapping::Mapped(command) => return Ok(command),
            WireCommandMapping::Unhandled(command) => command,
        };
        let command = match self.map_project_command(command)? {
            WireCommandMapping::Mapped(command) => return Ok(command),
            WireCommandMapping::Unhandled(command) => command,
        };
        let command = match self.map_agent_command(command)? {
            WireCommandMapping::Mapped(command) => return Ok(command),
            WireCommandMapping::Unhandled(command) => command,
        };
        Ok(match command {
            WireCommand::DeleteTerminal { terminal_id } => Command::DeleteTerminal { terminal_id },
            WireCommand::DeleteSession {
                session_id,
                delete_worktree,
                delete_branch,
            } => Command::BeginDeleteSession {
                session_id,
                delete_worktree,
                delete_branch,
            },
            WireCommand::PersistGlobalEnv { env } => Command::PersistGlobalEnv { env },
            WireCommand::ReloadConfig {} => Command::ReloadConfig,
            WireCommand::RecoverConfig {} => Command::RecoverConfig,
            // Rename, Reconnect, CheckoutProjectDefaultBranch, and
            // ChangeAgentProvider are NOT reconstructible into a single
            // `Command` — all need `&mut self` (rename persists in place;
            // reconnect tears down provider state and surfaces a launch view's
            // status synchronously; checkout inspects and switches branches
            // synchronously; change-provider persists + pins in place).
            // `apply_wire` intercepts them before this immutable mapping;
            // reaching here means that interception broke.
            WireCommand::StageFile { .. }
            | WireCommand::UnstageFile { .. }
            | WireCommand::DiscardFile { .. }
            | WireCommand::CommitChanges { .. }
            | WireCommand::Push { .. }
            | WireCommand::Pull { .. }
            | WireCommand::PullProject { .. }
            | WireCommand::ToggleAgentAutoReopen { .. }
            | WireCommand::UpdateProjectProvider { .. }
            | WireCommand::UpdateProjectAutoReopen { .. }
            | WireCommand::UpdateProjectStartupCommand { .. }
            | WireCommand::UpdateProjectEnv { .. }
            | WireCommand::AddProject { .. }
            | WireCommand::RemoveProject { .. }
            | WireCommand::DeleteProject { .. }
            | WireCommand::CreateStandaloneAgent { .. }
            | WireCommand::CreateAgent { .. }
            | WireCommand::ForkSession { .. }
            | WireCommand::CreateAgentFromWorktree { .. }
            | WireCommand::RenameSession { .. }
            | WireCommand::ReconnectSession { .. }
            | WireCommand::RerunStartupCommand { .. }
            | WireCommand::CheckoutProjectDefaultBranch { .. }
            | WireCommand::AddProjectCheckoutDefault { .. }
            | WireCommand::AddProjectCreateInitialCommit { .. }
            | WireCommand::AddProjectInitRepo { .. }
            | WireCommand::ChangeAgentProvider { .. }
            | WireCommand::CreateAgentFromPr { .. }
            | WireCommand::AttachPullRequest { .. }
            | WireCommand::ClearPullRequestOverride { .. }
            | WireCommand::ResumePullRequestAutodetection { .. }
            | WireCommand::SetChangesPaneVisible { .. }
            | WireCommand::SetInstanceIdentity { .. }
            | WireCommand::SetSettings(..)
            | WireCommand::ToggleRandomizedPetNameDefault {}
            | WireCommand::TogglePrBannerPosition {}
            | WireCommand::SetAgentSort { .. }
            | WireCommand::ToggleCopyOnSelect {}
            | WireCommand::ToggleGithubIntegration {}
            | WireCommand::RecheckGithub {}
            | WireCommand::ToggleAlwaysShowTabStrip {}
            | WireCommand::ToggleTabReachesAgent {}
            | WireCommand::SetTailscaleMode { .. }
            | WireCommand::KillSessionPty { .. }
            | WireCommand::DetachAgent { .. }
            | WireCommand::CloseAgentTab { .. }
            | WireCommand::ChangeAgentTabProvider { .. }
            | WireCommand::SetLastFocusedTab { .. } => {
                unreachable!(
                    "changes commands are mapped before the remaining wire_to_command dispatch; rename/reconnect/rerun-startup-command/checkout-default-branch/add-project-checkout-default/change-provider/create-agent-from-pr/set-changes-pane-visible/set-instance-identity/set-settings/toggle-randomized-pet-name-default/toggle-pr-banner-position/set-agent-sort/toggle-copy-on-select/toggle-github-integration/toggle-always-show-tab-strip/toggle-tab-reaches-agent/kill-session-pty/detach-agent/close-agent-tab/change-agent-tab-provider/set-last-focused-tab are handled in apply_wire before wire_to_command"
                )
            }
            WireCommand::ReorderSessions {
                project_id,
                session_ids,
            } => Command::ReorderSessions {
                project_id,
                session_ids,
            },
            WireCommand::ReorderAgents { session_ids } => Command::ReorderAgents { session_ids },
            WireCommand::ReorderProjects { project_ids } => {
                Command::ReorderProjects { project_ids }
            }
            WireCommand::ReorderTerminals { terminal_ids } => {
                Command::ReorderTerminals { terminal_ids }
            }
            WireCommand::RunMacro { target_id, name } => Command::RunMacro { target_id, name },
            WireCommand::UpdateMacros { entries } => Self::update_macros_command(entries)?,
            WireCommand::WatchChangedFiles { session_id } => {
                Command::WatchChangedFiles { session_id }
            }
        })
    }

    fn project_name(&self, project_id: &str) -> anyhow::Result<String> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))
    }

    /// The directory a CHANGES-PANEL mutation may run in: a managed worktree,
    ///
    /// (This replaced a plain "resolve the session's worktree" helper. Every
    /// wire command that used it was a git command, and each now goes through
    /// either this folder-driven gate or the branch-identity one. The PURE
    /// FILESYSTEM routes never went through the wire at all: they resolve the
    /// agent's directory in `crate::git_routes::resolve_worktree`, which is
    /// correct for a plain folder and stays ungated.)
    /// or a standalone agent's folder when that folder is itself a repository.
    ///
    /// Folder-driven, not agent-driven, which is the whole point: a standalone
    /// agent pointed at a repository stages and commits exactly like any other.
    /// When the folder is not a repository the refusal carries the folder's own
    /// quiet sentence, so the user is told "this folder has no git repository"
    /// rather than the "the repository is busy" the old error path produced once
    /// per poll. An unclassified folder fails CLOSED: a mutation on a guess is
    /// how a directory dux does not understand gets written to.
    fn changes_worktree(&self, session_id: &str) -> anyhow::Result<PathBuf> {
        let access = self
            .session_git_access(session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        if !access.mutations_allowed() {
            anyhow::bail!(
                "{}",
                access
                    .quiet_reason()
                    .unwrap_or("dux cannot work with git in this folder.")
            );
        }
        Ok(access.directory().to_path_buf())
    }

    /// List the project's managed worktrees that are currently adoptable as a
    /// new agent: managed by dux (under the worktrees root) and selectable (no
    /// live session, not the project checkout). Shells to git via
    /// `list_worktrees` + `classify_project_worktrees` — bounded plumbing reads,
    /// no working-tree writes — and is the single source of truth shared by the
    /// listing handler and the wire re-validation, so the client's path is always
    /// checked against a fresh classification rather than a stale snapshot.
    pub fn adoptable_managed_worktrees(
        &self,
        project: &Project,
    ) -> anyhow::Result<Vec<crate::worker::ProjectWorktreeEntry>> {
        let worktrees = crate::git::list_worktrees(std::path::Path::new(&project.path))?;
        Ok(crate::project_browser::classify_project_worktrees(
            project,
            &self.paths,
            &self.sessions,
            worktrees,
        )
        .into_iter()
        .filter(|entry| entry.is_selectable && entry.is_managed_by_dux)
        .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        sample_project, sample_session, sample_standalone_session, settle_gh_probe, test_engine,
    };
    use crate::engine::{DeleteTerminalView, InFlightKey};
    use crate::ids::TabId;
    use crate::model::AgentSession;
    use crate::statusline::StatusTone;
    use crate::worker::WorkerEvent;
    use std::path::Path;

    /// The folder is accepted whatever it contains: a plain directory with no
    /// repository anywhere near it is the ordinary case, and the add-project
    /// validator (which rejects exactly that) must not be in the way.
    #[test]
    fn creating_a_standalone_agent_accepts_a_folder_that_is_not_a_repository() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let outcome = engine.apply_wire(WireCommand::CreateStandaloneAgent {
            folder: folder.path().to_string_lossy().to_string(),
            name: String::new(),
            provider: None,
        });
        assert!(
            outcome.is_ok(),
            "a plain folder is the ordinary case, got {:?}",
            outcome.err().map(|e| e.to_string())
        );
    }

    /// Provider tools resume their conversation history PER DIRECTORY, so two
    /// standalone agents in one folder would silently resume each other's
    /// conversations. Refused at creation, canonically (a symlink to the same
    /// folder is the same folder), and the refusal is a signpost rather than a
    /// wall: it recommends the shape dux is actually built for.
    #[test]
    fn a_second_standalone_agent_on_the_same_folder_is_refused_with_a_signpost() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().to_string_lossy().to_string();
        engine
            .sessions
            .push(sample_standalone_session("sa1", &path));

        let message = match engine.apply_wire(WireCommand::CreateStandaloneAgent {
            folder: path.clone(),
            name: String::new(),
            provider: None,
        }) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("a second agent in the same folder must be refused"),
        };
        assert!(
            message.contains("already"),
            "it must say one is already there, got {message:?}"
        );
        assert!(
            message.contains("conversation"),
            "and why that matters, got {message:?}"
        );
        assert!(
            message.contains("as a project"),
            "and point at the multi-agent shape, got {message:?}"
        );
    }

    /// A standalone agent cannot be aimed at a directory ANOTHER AGENT already
    /// occupies, whichever kind that agent is.
    ///
    /// The refusal exists because coding CLIs resume their conversation history
    /// per directory, and that is just as true of a managed agent's worktree:
    /// pointing a standalone agent there would resume the managed agent's
    /// conversation. It is worse than a resumed conversation, too, because
    /// launching in an occupied worktree DETACHES the agent already there, and
    /// the survivor then blocks that worktree's deletion forever with a
    /// "still used by other agents" message.
    #[test]
    fn a_standalone_agent_cannot_be_aimed_at_a_managed_agents_worktree() {
        let (mut engine, tmp) = test_engine();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let mut managed = sample_session("s1", "p1", "feat");
        managed
            .workspace
            .as_managed_mut()
            .expect("managed")
            .worktree_path = worktree.to_string_lossy().to_string();
        engine.sessions.push(managed);

        // Reached by a different spelling, to pin the canonical comparison.
        let link = tmp.path().join("link-to-wt");
        std::os::unix::fs::symlink(&worktree, &link).unwrap();
        let message = match engine.apply_wire(WireCommand::CreateStandaloneAgent {
            folder: link.to_string_lossy().to_string(),
            name: String::new(),
            provider: None,
        }) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("a directory another agent occupies must be refused"),
        };
        assert!(
            message.contains("s1-title"),
            "the refusal must name who is already there, got {message:?}"
        );
        assert!(
            message.contains("conversation"),
            "and why it matters, got {message:?}"
        );
    }

    /// Two creates for the same folder cannot both get through, even before the
    /// first one's session exists to be seen.
    ///
    /// The one-agent-per-folder refusal reads `self.sessions`, and a session is
    /// not inserted until its provider has launched, which leaves a window the
    /// refusal cannot see into. What closes it is the process-global create
    /// lock: creates are serialised whole, and it is cleared in the same
    /// synchronous handler that inserts the session, so there is no moment when
    /// a create is in flight and its folder looks free.
    ///
    /// Pinned because the reasoning is the guarantee: nothing about the folder
    /// refusal itself would stop the second create.
    #[test]
    fn two_creates_for_one_folder_cannot_both_be_in_flight() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().to_string_lossy().to_string();
        let create = || WireCommand::CreateStandaloneAgent {
            folder: path.clone(),
            name: String::new(),
            provider: None,
        };

        engine
            .apply_wire(create())
            .expect("the first create starts");
        assert!(
            engine.is_in_flight(&InFlightKey::CreateAgent),
            "and holds the process-global create lock while it runs"
        );

        // No session exists yet: the folder refusal has nothing to find.
        assert!(engine.sessions.is_empty());
        let second = engine.apply_wire(create()).expect("the second is answered");
        let message = second
            .status
            .as_ref()
            .map(|s| s.message.clone())
            .unwrap_or_default();
        assert_eq!(
            second.status.as_ref().map(|s| s.tone.as_str()),
            Some("error"),
            "the second create must be refused, got {message:?}"
        );
        assert!(
            message.contains("already being created"),
            "and refused by the create lock, which is what covers this window: {message:?}"
        );
    }

    #[test]
    fn the_same_folder_reached_through_a_symlink_is_still_the_same_folder() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let holder = tempfile::tempdir().unwrap();
        let link = holder.path().join("link");
        std::os::unix::fs::symlink(folder.path(), &link).unwrap();
        engine.sessions.push(sample_standalone_session(
            "sa1",
            &folder.path().to_string_lossy(),
        ));

        let outcome = engine.apply_wire(WireCommand::CreateStandaloneAgent {
            folder: link.to_string_lossy().to_string(),
            name: String::new(),
            provider: None,
        });
        assert!(
            outcome.is_err(),
            "a symlink must not be a way around the one-agent-per-folder rule"
        );
    }

    /// Every row label used to fall through the branch name, which does not
    /// exist here, so a non-empty title is a hard invariant rather than an
    /// option. The rule is a pure function so both surfaces derive the same
    /// name from the same folder.
    #[test]
    fn a_standalone_agent_without_a_typed_name_is_named_after_its_folder() {
        let name = crate::git::standalone_agent_title("", Path::new("/home/ada/My Notes! (2026)"));
        assert!(!name.is_empty(), "a standalone agent always has a name");
        assert!(
            name.contains("Notes"),
            "and it comes from the folder, got {name:?}"
        );
    }

    /// A folder whose name sanitizes away to nothing still yields a name: the
    /// invariant is non-empty, not "whatever the sanitizer produced".
    #[test]
    fn a_folder_whose_name_sanitizes_to_nothing_still_gets_a_name() {
        for path in ["/home/ada/!!!", "/", "/home/ada/   "] {
            let name = crate::git::standalone_agent_title("", Path::new(path));
            assert!(!name.is_empty(), "{path} produced an empty name");
        }
    }

    /// Folder names legally contain characters a branch name cannot, and a
    /// standalone agent has no branch, so the branch validator must not be
    /// applied to a typed name here.
    #[test]
    fn a_typed_name_is_not_held_to_the_branch_naming_rules() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let outcome = engine.apply_wire(WireCommand::CreateStandaloneAgent {
            folder: folder.path().to_string_lossy().to_string(),
            name: "Sort the downloads!".to_string(),
            provider: None,
        });
        assert!(
            outcome.is_ok(),
            "no branch is created, so no branch rule applies: {:?}",
            outcome.err().map(|e| e.to_string())
        );
        assert_eq!(
            crate::git::standalone_agent_title("Sort the downloads!", Path::new("/anywhere")),
            "Sort the downloads!",
            "a typed name is used verbatim once trimmed"
        );
    }

    /// Hiding the buttons is not an answer when the same action is an HTTP
    /// route: a standalone agent's id must not be able to reach a real push in
    /// the user's folder from a command line. Every branch-identity command
    /// refuses it on the server, with a purposeful sentence rather than an
    /// accidental git error about a repository nobody named.
    #[test]
    fn the_branch_identity_commands_refuse_a_standalone_agent_on_the_server() {
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_standalone_session("sa1", "/home/someone/notes"));

        for command in [
            WireCommand::Push {
                session_id: "sa1".to_string(),
            },
            WireCommand::Pull {
                session_id: "sa1".to_string(),
            },
            WireCommand::ForkSession {
                session_id: "sa1".to_string(),
                name: "copy".to_string(),
            },
        ] {
            let label = format!("{command:?}");
            let message = match engine.wire_to_command(command) {
                Err(err) => err.to_string(),
                Ok(_) => panic!("{label} must be refused for a standalone agent"),
            };
            assert!(message.contains("standalone agent"), "{label}: {message:?}");
            assert!(message.contains("no branch"), "{label}: {message:?}");
            // A refusal with no way forward is where a user gets stuck.
            assert!(
                message.contains("as a project"),
                "{label} must point somewhere: {message:?}"
            );
        }
    }

    /// The changes routes are FOLDER driven, not agent driven: they work in a
    /// standalone agent's folder when it is a repository, and refuse when it is
    /// not, saying why in the folder's own terms.
    #[test]
    fn the_changes_routes_follow_the_folder_and_not_the_agent() {
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().unwrap();
        engine.sessions.push(sample_standalone_session(
            "sa1",
            &repo.path().to_string_lossy(),
        ));

        // Unprobed: quiet, and no mutation on a guess.
        let message = match engine.wire_to_command(WireCommand::StageFile {
            session_id: "sa1".to_string(),
            path: "a.txt".to_string(),
        }) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("a folder dux has not classified must not be staged into"),
        };
        assert!(message.contains("still looking"), "{message:?}");

        // A real repository: staging is ordinary work again.
        engine
            .folder_repo_statuses
            .insert("sa1".to_string(), crate::git::FolderRepoStatus::WorkingRepo);
        assert!(
            engine
                .wire_to_command(WireCommand::StageFile {
                    session_id: "sa1".to_string(),
                    path: "a.txt".to_string(),
                })
                .is_ok(),
            "a standalone agent pointed at a repository gets a real changes panel"
        );
    }

    /// A directory two agents share is preserved, and "share" is decided on
    /// CANONICAL paths.
    ///
    /// This is the one guard between "delete agent A and remove its worktree"
    /// and destroying a directory another agent is running in. The worktree
    /// manager's occupancy check compares canonically; this one compared the
    /// stored strings, so a standalone agent reaching the same directory by a
    /// different spelling was invisible to it and the removal went ahead with
    /// the `--force` flag.
    #[test]
    fn a_shared_directory_is_preserved_even_when_the_other_agent_spells_it_differently() {
        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        // The same directory, reached through a symlink.
        let link = tmp.path().join("link-to-wt");
        std::os::unix::fs::symlink(&worktree, &link).unwrap();

        engine
            .projects
            .push(sample_project("p1", repo.to_string_lossy().as_ref()));
        let mut managed = sample_session("s1", "p1", "feat");
        managed
            .workspace
            .as_managed_mut()
            .expect("managed")
            .worktree_path = worktree.to_string_lossy().to_string();
        engine.sessions.push(managed);
        engine
            .sessions
            .push(sample_standalone_session("sa1", &link.to_string_lossy()));

        let outcome = match engine.do_delete_session("s1", true, None) {
            Ok(Some(outcome)) => outcome,
            other => panic!("the delete must proceed, got {:?}", other.is_ok()),
        };
        assert!(
            matches!(outcome.removal, WorktreeRemoval::SkippedForSiblings),
            "the directory is occupied by another agent, so it must be preserved"
        );
        assert!(
            worktree.exists(),
            "and it must still be on disk: another agent is running in it"
        );
    }

    /// THE PIN. Deleting a standalone agent removes dux's record of it and
    /// nothing else, and the message says so in as many words. Nothing about
    /// worktrees, nothing about branches, and above all no claim that anything
    /// on disk was removed.
    #[test]
    fn deleting_a_standalone_agent_says_the_folder_was_left_alone() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let folder_path = folder.path().to_string_lossy().to_string();
        engine
            .sessions
            .push(sample_standalone_session("sa1", &folder_path));

        let outcome = match engine.do_delete_session("sa1", false, None) {
            Ok(Some(outcome)) => outcome,
            Ok(None) => panic!("the session existed"),
            Err(err) => panic!("delete must succeed: {err}"),
        };
        let message = delete_session_status_message(&outcome.finish, &outcome.removal);

        assert!(
            message.contains("folder"),
            "the message must name what was left alone, got {message:?}"
        );
        assert!(
            !message.to_lowercase().contains("worktree"),
            "a standalone agent has no worktree to mention, got {message:?}"
        );
        assert!(
            !message.to_lowercase().contains("branch"),
            "nor a branch, got {message:?}"
        );
        assert!(
            folder.path().exists(),
            "and the folder itself must still be there"
        );
    }

    /// Asking for the worktree to be removed on a standalone id is refused out
    /// loud. It is never quietly ignored: silent success theater about a
    /// destructive request is how a user comes to believe dux cleaned
    /// something up.
    #[test]
    fn asking_to_delete_a_standalone_agents_folder_is_refused_not_ignored() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().unwrap();
        let folder_path = folder.path().to_string_lossy().to_string();
        engine
            .sessions
            .push(sample_standalone_session("sa1", &folder_path));

        let message = match engine.do_delete_session("sa1", true, None) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("a destructive request dux will not honour must be refused"),
        };
        assert!(message.contains("standalone agent"), "got {message:?}");
        assert!(
            message.contains("never"),
            "the refusal must state the rule, got {message:?}"
        );
        assert!(
            engine.sessions.iter().any(|s| s.id == "sa1"),
            "and change nothing"
        );
        assert!(folder.path().exists());
    }

    /// The web's delete line must account for BOTH branches the removal
    /// targets. It used to say only "removed its worktree", which is true and
    /// incomplete: the branch went too, and for a drifted agent so did a second
    /// branch the message never mentioned.
    #[test]
    fn delete_message_names_the_branch_and_the_drifted_original() {
        fn message(branch: &str, initial: &str, branches: crate::git::RemoveResult) -> String {
            let mut session = sample_session("s1", "p1", branch);
            session.title = Some("agent-one".to_string());
            session
                .workspace
                .as_managed_mut()
                .expect("managed test session")
                .initial_branch = initial.to_string();
            let outcome = FinishDeleteSessionOutcome {
                session,
                project: None,
                other_sessions_on_worktree: false,
                project_still_has_sessions: false,
            };
            delete_session_status_message(
                &outcome,
                &WorktreeRemoval::Performed {
                    branches: crate::engine::RemovedBranches::Deleted(branches),
                },
            )
        }

        // No drift: one branch, named.
        assert_eq!(
            message("feat", "feat", crate::git::RemoveResult::default()),
            "Deleted agent \"agent-one\", removed its worktree and deleted its branch \"feat\"."
        );
        // Drifted: the birth branch went too, and the line says so.
        assert_eq!(
            message(
                "drifted",
                "born-here",
                crate::git::RemoveResult {
                    branch: crate::git::BranchDeletion::Deleted,
                    initial_branch: Some(crate::git::BranchDeletion::Deleted),
                },
            ),
            "Deleted agent \"agent-one\", removed its worktree and deleted its branch \
             \"drifted\". Its original branch \"born-here\" was deleted too."
        );
        // Neither branch was dux's to delete: the message must not claim it did.
        assert_eq!(
            message(
                "drifted",
                "born-here",
                crate::git::RemoveResult {
                    branch: crate::git::BranchDeletion::AlreadyGone,
                    initial_branch: Some(crate::git::BranchDeletion::AlreadyGone),
                },
            ),
            "Deleted agent \"agent-one\" and removed its worktree. Its branch \"drifted\" was \
             already gone. Its original branch \"born-here\" was already gone."
        );
    }

    /// The keep path: the worktree went and the branches stayed. The message
    /// must name every kept branch, say why each one was kept, and hand over
    /// `git branch -D`, because once the worktree is gone no dux surface can
    /// reach the branch.
    #[test]
    fn delete_message_names_every_branch_it_deliberately_kept() {
        fn message(
            branch: &str,
            initial: &str,
            provenance: crate::model::BranchProvenance,
        ) -> String {
            let mut session = sample_session("s1", "p1", branch);
            session.title = Some("agent-one".to_string());
            session
                .workspace
                .as_managed_mut()
                .expect("managed test session")
                .initial_branch = initial.to_string();
            let outcome = FinishDeleteSessionOutcome {
                session,
                project: None,
                other_sessions_on_worktree: false,
                project_still_has_sessions: false,
            };
            delete_session_status_message(
                &outcome,
                &WorktreeRemoval::Performed {
                    branches: crate::engine::RemovedBranches::Kept(
                        crate::model::BranchKeptReason::NotDuxs(provenance),
                    ),
                },
            )
        }

        assert_eq!(
            message(
                "develop",
                "develop",
                crate::model::BranchProvenance::AttachedExisting
            ),
            "Deleted agent \"agent-one\" and removed its worktree. Its branch \"develop\" \
             existed before this agent and was kept. Delete it yourself with \
             git branch -D \"develop\" if you no longer need it."
        );
        // Drifted: two branches survive, and "existed before this agent" is
        // false of the one the drift created, so each gets its own reason.
        assert_eq!(
            message(
                "feature-x",
                "develop",
                crate::model::BranchProvenance::AttachedExisting
            ),
            "Deleted agent \"agent-one\" and removed its worktree. Its branch \"feature-x\" \
             was created inside this agent's worktree and was kept, and its branch \
             \"develop\" existed before this agent and was kept. Delete either yourself \
             with git branch -D \"feature-x\" or git branch -D \"develop\" if you no longer \
             need them."
        );
        // An adopted agent's branch came with the worktree, which is a different
        // sentence from "existed before this agent".
        assert_eq!(
            message("main", "main", crate::model::BranchProvenance::Adopted),
            "Deleted agent \"agent-one\" and removed its worktree. Its branch \"main\" \
             came with the worktree this agent adopted and was kept. Delete it yourself \
             with git branch -D \"main\" if you no longer need it."
        );
    }

    #[test]
    fn wire_status_without_scope_field_deserializes_to_all() {
        // An older peer / the TUI emits a status JSON with no `scope` key; the
        // `#[serde(default)]` must fill in `StatusScope::All` (broadcast) so the
        // status still reaches every client exactly as before scoping existed.
        let json = r#"{"tone":"info","message":"Committed."}"#;
        let status: WireStatus = serde_json::from_str(json).expect("deserialize");
        assert_eq!(status.scope, StatusScope::All);
        assert_eq!(status.tone, "info");
        assert_eq!(status.message, "Committed.");
        assert_eq!(status.key, None);

        // A scoped status round-trips through serde to the right connection.
        let scoped = WireStatus::new("busy", "Pushing…")
            .with_scope(StatusScope::Connection("conn-7".to_string()));
        let round: WireStatus =
            serde_json::from_str(&serde_json::to_string(&scoped).expect("serialize"))
                .expect("deserialize");
        assert_eq!(round.scope, StatusScope::Connection("conn-7".to_string()));
    }

    #[test]
    fn wire_command_deserializes_from_json_envelope() {
        let json = r#"{"command":"stage_file","args":{"session_id":"s1","path":"a.txt"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::StageFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string()
            }
        );
    }

    #[test]
    fn wire_delete_terminal_deserializes() {
        let json = r#"{"command":"delete_terminal","args":{"terminal_id":"term-1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::DeleteTerminal {
                terminal_id: "term-1".to_string()
            }
        );
    }

    #[test]
    fn wire_statuses_reports_closed_terminal() {
        let r = EventReaction::DeleteTerminalView(Box::new(DeleteTerminalView {
            terminal_id: "term-1".to_string(),
            label: Some("Terminal 1".to_string()),
        }));
        let s = wire_statuses_from_reaction(&r);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tone, "info");
        assert!(s[0].message.contains("Closed terminal \"Terminal 1\""));
    }

    #[test]
    fn wire_to_command_resolves_worktree_from_session() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let cmd = engine
            .wire_to_command(WireCommand::StageFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::StageFile {
                worktree_path,
                path,
            } => {
                assert_eq!(worktree_path, Path::new("/tmp/s1-worktree"));
                assert_eq!(path, "a.txt");
            }
            _ => panic!("expected Command::StageFile variant"),
        }
    }

    #[test]
    fn wire_discard_file_deserializes() {
        let json = r#"{"command":"discard_file","args":{"session_id":"s1","path":"a.txt"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string()
            }
        );
    }

    /// Build a session whose worktree points at a real temp git repo so the
    /// discard classifier can run `git status` against it.
    fn session_in_repo(id: &str, repo: &Path) -> AgentSession {
        let mut s = sample_session(id, "p1", "feat");
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.to_string_lossy().into_owned();
        s
    }

    #[test]
    fn wire_to_command_discard_derives_untracked_for_untracked_file() {
        // init_repo leaves a.txt untracked (never committed).
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let cmd = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DiscardFile {
                worktree_path,
                path,
                is_untracked,
            } => {
                assert_eq!(worktree_path, repo.path());
                assert_eq!(path, "a.txt");
                assert!(is_untracked, "untracked file must be classified untracked");
            }
            _ => panic!("expected Command::DiscardFile variant"),
        }
    }

    #[test]
    fn wire_to_command_discard_derives_tracked_for_modified_file() {
        // init_repo_with_commit commits a.txt; modify it so it has an unstaged
        // (tracked) change.
        let repo = init_repo_with_commit();
        std::fs::write(repo.path().join("a.txt"), "changed\n").expect("modify file");
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let cmd = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DiscardFile { is_untracked, .. } => {
                assert!(!is_untracked, "modified tracked file must not be untracked");
            }
            _ => panic!("expected Command::DiscardFile variant"),
        }
    }

    #[test]
    fn wire_to_command_discard_rejects_staged_file() {
        // Commit a.txt, modify it, then STAGE the modification: it is now purely
        // staged with no remaining working-tree change. The TUI blocks this.
        let repo = init_repo_with_commit();
        std::fs::write(repo.path().join("a.txt"), "staged change\n").expect("modify file");
        let ok = crate::git::test_support::git_command()
            .args(["add", "a.txt"])
            .current_dir(repo.path())
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git add failed");
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let err = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        // Mirrors the TUI's "Unstage the file first to discard changes." copy.
        assert!(
            err.to_string()
                .contains("Unstage the file first to discard changes."),
            "got: {err}"
        );
    }

    #[test]
    fn wire_to_command_discard_rejects_unchanged_file() {
        // a.txt is committed and clean — nothing to discard.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let err = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("No unstaged changes to discard"),
            "got: {err}"
        );
    }

    #[test]
    fn wire_pull_deserializes() {
        let json = r#"{"command":"pull","args":{"session_id":"s1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::Pull {
                session_id: "s1".to_string()
            }
        );
    }

    #[test]
    fn wire_to_command_pull_resolves_worktree() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let cmd = engine
            .wire_to_command(WireCommand::Pull {
                session_id: "s1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::Pull {
                repo_path,
                target: PullTarget::Session,
                ..
            } => {
                assert_eq!(repo_path, Path::new("/tmp/s1-worktree"));
            }
            _ => panic!("expected Command::Pull variant with Session target"),
        }
    }

    #[test]
    fn wire_pull_project_deserializes() {
        let json = r#"{"command":"pull_project","args":{"project_id":"p1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::PullProject {
                project_id: "p1".to_string()
            }
        );
    }

    #[test]
    fn wire_rerun_startup_command_deserializes() {
        let json = r#"{"command":"rerun_startup_command","args":{"session_id":"s1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::RerunStartupCommand {
                session_id: "s1".to_string()
            }
        );
    }

    #[test]
    fn wire_rerun_startup_command_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::RerunStartupCommand {
                session_id: "ghost".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn wire_rerun_startup_command_without_startup_command_errors() {
        let (mut engine, _tmp) = test_engine();
        // sample_project has startup_command = None.
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .apply_wire(WireCommand::RerunStartupCommand {
                session_id: "s1".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("does not have a startup command"),
            "err: {err}"
        );
    }

    #[test]
    fn wire_rerun_startup_command_runs_and_reports_busy_then_final() {
        let (mut engine, _tmp) = test_engine();
        // A real worktree directory the command can `cd` into.
        let worktree = tempfile::tempdir().expect("worktree dir");
        let mut project = sample_project("p1", "/repo");
        project.startup_command = Some("printf hi".to_string());
        engine.projects.push(project);
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        // The dispatch returns the pending Busy immediately.
        let outcome = engine
            .apply_wire(WireCommand::RerunStartupCommand {
                session_id: "s1".to_string(),
            })
            .expect("dispatch ok");
        let busy = outcome.status.expect("a busy status");
        assert!(
            busy.message.contains("Rerunning startup command"),
            "busy: {}",
            busy.message
        );

        // The off-thread run finishes and ships its keyed final via StatusOpCompleted.
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("status-op completion event");
        match event {
            crate::worker::WorkerEvent::StatusOpCompleted { resolved } => {
                // Same key correlates the busy and its final.
                assert_eq!(resolved.key, busy.key.expect("busy carries a key"));
            }
            _ => panic!("expected a StatusOpCompleted worker event"),
        }

        // The run wrote a log file under the agent's startup-command-log dir.
        let logs = crate::startup::list_agent_logs(&engine.paths, "p1", "s1").expect("list logs");
        assert!(
            !logs.is_empty(),
            "expected a startup command log to be written"
        );
    }

    #[test]
    fn wire_to_command_pull_project_mirrors_tui_refresh() {
        // sample_project("p1", "/repo") has leading_branch Some("main") and
        // name "p1-name"; the constructed Pull must target the project's source
        // checkout path with PullTarget::Project and the TUI's exact messages.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let cmd = engine
            .wire_to_command(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::Pull {
                repo_path,
                target,
                busy_message,
                already_running_message,
            } => {
                assert_eq!(repo_path, Path::new("/repo"));
                match target {
                    PullTarget::Project {
                        project_id,
                        project_name,
                        leading_branch,
                    } => {
                        assert_eq!(project_id, "p1");
                        assert_eq!(project_name, "p1-name");
                        assert_eq!(leading_branch.as_deref(), Some("main"));
                    }
                    PullTarget::Session => panic!("expected PullTarget::Project"),
                }
                assert_eq!(
                    busy_message,
                    "Refreshing project \"p1-name\" from remote\u{2026}"
                );
                assert_eq!(
                    already_running_message,
                    "Project refresh already in progress for \"p1-name\". Wait for the current pull to finish."
                );
            }
            _ => panic!("expected Command::Pull variant with Project target"),
        }
    }

    #[test]
    fn wire_to_command_pull_project_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let err = engine
            .wire_to_command(WireCommand::PullProject {
                project_id: "ghost".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn wire_to_command_pull_project_path_missing_errors() {
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/repo");
        project.path_missing = true;
        engine.projects.push(project);
        let err = engine
            .wire_to_command(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        // Mirrors the TUI's `refresh_selected_project` path_missing warning.
        assert!(
            err.to_string()
                .contains("Cannot refresh: path not found for \"p1-name\""),
            "err: {err}"
        );
    }

    #[test]
    fn apply_wire_pull_project_blocks_repeat_while_running() {
        // First dispatch spawns the pull worker (busy arrives on the async
        // status stream, so the synchronous outcome is empty); the second
        // dispatch hits the in-flight guard and surfaces the TUI's
        // already-running warning synchronously.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        let first = engine
            .apply_wire(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .expect("first project pull");
        assert!(
            first.status.is_none(),
            "first pull busy is async, not a synchronous outcome: {:?}",
            first.status
        );
        assert!(engine.is_in_flight(&InFlightKey::Pull("/repo".to_string())));

        let second = engine
            .apply_wire(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .expect("repeat project pull should not error");
        let status = second.status.expect("in-flight warning status");
        assert_eq!(status.tone, "warning");
        assert!(
            status.message.contains("already in progress"),
            "unexpected message: {}",
            status.message
        );
    }

    #[test]
    fn wire_to_command_unknown_session_errors() {
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::Push {
            session_id: "ghost".to_string(),
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn apply_wire_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let res = engine.apply_wire(WireCommand::Push {
            session_id: "ghost".to_string(),
        });
        assert!(res.is_err());
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "hello\n").expect("write file");
        dir
    }

    #[test]
    fn wire_delete_session_deserializes() {
        let json =
            r#"{"command":"delete_session","args":{"session_id":"s1","delete_worktree":true}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::DeleteSession {
                session_id: "s1".to_string(),
                delete_worktree: true,
                delete_branch: None,
            },
            "an absent branch answer must mean nobody was asked, not \"keep it\": that is what \
             keeps a caller written before the checkbox existed behaving as it always did"
        );
    }

    #[test]
    fn wire_delete_session_carries_an_explicit_branch_answer() {
        for (json_value, expected) in [("true", Some(true)), ("false", Some(false))] {
            let json = format!(
                r#"{{"command":"delete_session","args":{{"session_id":"s1","delete_worktree":true,"delete_branch":{json_value}}}}}"#
            );
            let cmd: WireCommand = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                cmd,
                WireCommand::DeleteSession {
                    session_id: "s1".to_string(),
                    delete_worktree: true,
                    delete_branch: expected,
                }
            );
        }
    }

    #[test]
    fn wire_persist_global_env_deserializes() {
        let json = r#"{"command":"persist_global_env","args":{"env":{"FOO":"bar"}}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            WireCommand::PersistGlobalEnv { env } => {
                assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
            }
            _ => panic!("expected WireCommand::PersistGlobalEnv variant"),
        }
    }

    #[test]
    fn wire_update_project_provider_deserializes() {
        let json = r#"{"command":"update_project_provider","args":{"project_id":"p1","provider":"codex"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateProjectProvider {
                project_id: "p1".to_string(),
                provider: Some("codex".to_string()),
            }
        );
    }

    #[test]
    fn wire_update_project_auto_reopen_deserializes() {
        let json = r#"{"command":"update_project_auto_reopen","args":{"project_id":"p1","auto_reopen_agents":true}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateProjectAutoReopen {
                project_id: "p1".to_string(),
                auto_reopen_agents: Some(true),
            }
        );
    }

    #[test]
    fn wire_update_project_startup_command_deserializes() {
        let json = r#"{"command":"update_project_startup_command","args":{"project_id":"p1","startup_command":"echo hi"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateProjectStartupCommand {
                project_id: "p1".to_string(),
                startup_command: Some("echo hi".to_string()),
            }
        );
    }

    #[test]
    fn wire_update_project_env_deserializes() {
        let json =
            r#"{"command":"update_project_env","args":{"project_id":"p1","env":{"FOO":"bar"}}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            WireCommand::UpdateProjectEnv { project_id, env } => {
                assert_eq!(project_id, "p1");
                assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
            }
            _ => panic!("expected WireCommand::UpdateProjectEnv variant"),
        }
    }

    #[test]
    fn wire_reload_config_deserializes_with_empty_args_object() {
        // The frontend sends `args: {}` through the generic command envelope, and
        // the server always re-includes the `args` key when reconstructing the
        // envelope. The empty struct variant deserializes from that map form. (A
        // true unit variant would reject the `args:{}` map; the empty struct
        // variant requires the `args` key, which is exactly what the wire carries.)
        let json = r#"{"command":"reload_config","args":{}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::ReloadConfig {});
    }

    #[test]
    fn wire_recover_config_deserializes_with_empty_args_object() {
        let json = r#"{"command":"recover_config","args":{}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::RecoverConfig {});
    }

    #[test]
    fn wire_set_changes_pane_visible_deserializes() {
        let json = r#"{"command":"set_changes_pane_visible","args":{"visible":false}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::SetChangesPaneVisible { visible: false });
    }

    #[test]
    fn apply_wire_set_changes_pane_visible_persists() {
        let (mut engine, _tmp) = test_engine();
        assert!(engine.config.ui.show_changes_pane);

        // Toggle off — the write is lazy (fire-and-forget via config_writer).
        engine
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: false })
            .expect("apply set_changes_pane_visible");
        assert!(
            !engine.config.ui.show_changes_pane,
            "in-memory value must flip immediately"
        );

        // Flush the lazy queue and verify the change reached disk.
        engine.config_writer.flush();
        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains("show_changes_pane = false"),
            "flushed config must contain show_changes_pane = false, got:\n{disk}"
        );

        // Toggle back on — in-memory must flip again.
        engine
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: true })
            .expect("apply toggle back");
        assert!(
            engine.config.ui.show_changes_pane,
            "in-memory value must flip back"
        );

        // Idempotent: repeating the same value is a no-op that still succeeds.
        engine
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: true })
            .expect("apply idempotent");
        assert!(engine.config.ui.show_changes_pane);
    }

    #[test]
    fn set_changes_pane_visible_hidden_message_points_at_reachable_controls() {
        // Hiding the pane unmounts it, and the pane's own actions menu with it,
        // so a hide message pointing at "the Changes actions menu" named a
        // control the user could no longer reach. The reopen affordances that
        // actually survive the hide are the header's show button and the
        // Preferences row; the message must name those and not the vanished
        // menu.
        let (mut engine, _tmp) = test_engine();
        let outcome = engine
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: false })
            .expect("hide changes pane");
        let status = outcome.status.expect("a status message");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("show button in the header"),
            "hidden message must point at the header show button, got: {}",
            status.message
        );
        assert!(
            status.message.contains("Preferences"),
            "hidden message must point at Preferences, got: {}",
            status.message
        );
        assert!(
            !status.message.contains("Changes actions menu"),
            "hidden message must not point at the menu that vanished with the pane, got: {}",
            status.message
        );

        // The idempotent already-hidden branch answers a user staring at the
        // same paneless screen, so it needs the same reopen directions as the
        // hide itself.
        let repeat = engine
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: false })
            .expect("hide again while already hidden");
        let repeat_status = repeat.status.expect("an already-hidden status");
        assert_eq!(repeat_status.tone, "info");
        assert!(
            repeat_status.message.contains("show button in the header"),
            "already-hidden message must point at the header show button, got: {}",
            repeat_status.message
        );
        assert!(
            repeat_status.message.contains("Preferences"),
            "already-hidden message must point at Preferences, got: {}",
            repeat_status.message
        );
    }

    #[test]
    fn wire_toggle_commands_deserialize() {
        for (json, expected) in [
            (
                r#"{"command":"toggle_randomized_pet_name_default","args":{}}"#,
                WireCommand::ToggleRandomizedPetNameDefault {},
            ),
            (
                r#"{"command":"toggle_pr_banner_position","args":{}}"#,
                WireCommand::TogglePrBannerPosition {},
            ),
            (
                r#"{"command":"toggle_github_integration","args":{}}"#,
                WireCommand::ToggleGithubIntegration {},
            ),
            (
                r#"{"command":"toggle_copy_on_select","args":{}}"#,
                WireCommand::ToggleCopyOnSelect {},
            ),
            (
                r#"{"command":"toggle_always_show_tab_strip","args":{}}"#,
                WireCommand::ToggleAlwaysShowTabStrip {},
            ),
            (
                r#"{"command":"toggle_tab_reaches_agent","args":{}}"#,
                WireCommand::ToggleTabReachesAgent {},
            ),
        ] {
            let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
            assert_eq!(cmd, expected);
        }
    }

    #[test]
    fn apply_wire_toggle_randomized_pet_name_default_flips_and_persists() {
        let (mut engine, _tmp) = test_engine();
        let start = engine.config.defaults.enable_randomized_pet_name_by_default;

        engine
            .apply_wire(WireCommand::ToggleRandomizedPetNameDefault {})
            .expect("apply toggle");
        assert_eq!(
            engine.config.defaults.enable_randomized_pet_name_by_default, !start,
            "in-memory value must flip immediately"
        );

        engine.config_writer.flush();
        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains(&format!(
                "enable_randomized_pet_name_by_default = {}",
                !start
            )),
            "flushed config must carry the flipped value, got:\n{disk}"
        );

        // A second toggle returns to the starting value (the server owns state).
        engine
            .apply_wire(WireCommand::ToggleRandomizedPetNameDefault {})
            .expect("apply toggle back");
        assert_eq!(
            engine.config.defaults.enable_randomized_pet_name_by_default,
            start
        );
    }

    #[test]
    fn apply_wire_set_agent_sort_persists_valid_and_rejects_unknown() {
        let (mut engine, _tmp) = test_engine();
        assert_eq!(engine.config.ui.agent_sort, "active");

        // A valid mode is accepted and persisted.
        let outcome = engine
            .apply_wire(WireCommand::SetAgentSort {
                sort: "manual".to_string(),
            })
            .expect("apply set-agent-sort");
        assert_eq!(engine.config.ui.agent_sort, "manual");
        assert_eq!(outcome.status.map(|s| s.tone), Some("info".to_string()));

        // The descending-name mode is a valid value too (the TUI can set it even
        // though the web picker does not offer it).
        let outcome = engine
            .apply_wire(WireCommand::SetAgentSort {
                sort: "name_desc".to_string(),
            })
            .expect("apply set-agent-sort (name_desc)");
        assert_eq!(engine.config.ui.agent_sort, "name_desc");
        assert_eq!(outcome.status.map(|s| s.tone), Some("info".to_string()));

        engine.config_writer.flush();
        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains("agent_sort = \"name_desc\""),
            "flushed config must carry the sort, got:\n{disk}"
        );

        // An unknown mode is rejected with an error tone and leaves the value alone.
        let outcome = engine
            .apply_wire(WireCommand::SetAgentSort {
                sort: "bogus".to_string(),
            })
            .expect("apply set-agent-sort (invalid)");
        assert_eq!(outcome.status.map(|s| s.tone), Some("error".to_string()));
        assert_eq!(engine.config.ui.agent_sort, "name_desc");
    }

    #[test]
    fn apply_wire_toggle_pr_banner_position_swaps_top_and_bottom() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.pr_banner_position = "top".to_string();

        engine
            .apply_wire(WireCommand::TogglePrBannerPosition {})
            .expect("apply toggle");
        assert_eq!(engine.config.ui.pr_banner_position, "bottom");

        engine
            .apply_wire(WireCommand::TogglePrBannerPosition {})
            .expect("apply toggle back");
        assert_eq!(engine.config.ui.pr_banner_position, "top");

        engine.config_writer.flush();
        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains("pr_banner_position = \"top\""),
            "flushed config must carry the position, got:\n{disk}"
        );
    }

    /// The lifecycle rule, counted rather than merely observed: an off-to-on
    /// site launches the probe and does NOTHING else, and the probe's own
    /// completion arms pull-request work exactly once.
    ///
    /// Counting is the point. Acting on the PRE-probe status meant an enable
    /// launched a refresh from a stale answer, and the completion then launched
    /// a second refresh AND another poller. `spawn_pr_sync_worker` had no
    /// single-instance guard and the running poller reads its kill switch only
    /// once every few seconds, so a fast off-then-on left the old poller alive
    /// and created a permanent second one. A test that only asserted "a poller
    /// exists" would have passed with three of them.
    #[test]
    fn a_rapid_off_on_off_on_arms_one_poller_and_one_refresh_per_enable() {
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program =
            crate::gh::probe_test_support::stand_in_gh_serving(dir.path(), &["github.com"]).into();
        engine.github_integration_enabled = false;
        engine.config.ui.github_integration = false;

        let counts = |engine: &Engine| {
            (
                engine.pr_sync.refresh_starts(),
                engine.pr_sync.poller_starts(),
            )
        };
        let toggle = |engine: &mut Engine| {
            engine
                .apply_wire(WireCommand::ToggleGithubIntegration {})
                .expect("toggle github integration");
        };

        toggle(&mut engine);
        let after_first_enable = counts(&engine);
        settle_gh_probe(&mut engine);
        let after_first_probe = counts(&engine);
        assert_eq!(engine.gh_status, crate::model::GhStatus::Available);

        toggle(&mut engine);
        assert!(!engine.pr_sync.is_armed(), "disabling disarms immediately");

        // Straight back on, inside the poller's read window.
        toggle(&mut engine);
        let after_second_enable = counts(&engine);
        settle_gh_probe(&mut engine);
        let after_second_probe = counts(&engine);

        assert_eq!(
            (
                after_first_enable,
                after_first_probe,
                after_second_enable,
                after_second_probe,
            ),
            ((0, 0), (1, 1), (1, 1), (2, 1)),
            "(refreshes, pollers) after: first enable, its probe, second enable, its probe. \
             An enable must arm nothing on its own, each probe must arm exactly one refresh, \
             and there must never be a second poller.",
        );
    }

    /// A success followed by a decisive failure must leave ZERO pollers armed.
    /// The handler used to change the status and leave the arm flag alone, so
    /// work armed from the older, better answer went on polling `gh` while the
    /// interface said GitHub was unavailable.
    #[test]
    fn a_success_followed_by_a_decisive_failure_leaves_zero_pollers_armed() {
        for outcome in [
            crate::gh::GhProbe::Decided {
                available: false,
                policy: crate::gh::GithubHostPolicy::Hosts(Default::default()),
            },
            crate::gh::GhProbe::NotInstalled,
        ] {
            let (mut engine, _tmp) = test_engine();
            engine.github_integration_enabled = true;
            engine.process_worker_event(crate::worker::WorkerEvent::GhStatusChecked {
                generation: engine.gh_probe.generation,
                outcome: crate::gh::GhProbe::Decided {
                    available: true,
                    policy: crate::gh::GithubHostPolicy::Hosts(
                        ["github.com".to_string()].into_iter().collect(),
                    ),
                },
            });
            assert!(engine.pr_sync.is_armed(), "a success arms the work");
            assert_eq!(engine.pr_sync.poller_starts(), 1);

            engine.process_worker_event(crate::worker::WorkerEvent::GhStatusChecked {
                generation: engine.gh_probe.generation,
                outcome: outcome.clone(),
            });

            assert!(
                !engine.pr_sync.is_armed(),
                "a decisive {outcome:?} must disarm pull-request work",
            );
            assert_eq!(
                engine.pr_sync.poller_starts(),
                1,
                "and must not have started another poller on the way",
            );
        }
    }

    /// The mirror image: a TRANSIENT result decides nothing, so it must leave
    /// the armed work alone rather than tearing it down.
    #[test]
    fn a_transient_failure_after_a_success_leaves_the_work_armed() {
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.process_worker_event(crate::worker::WorkerEvent::GhStatusChecked {
            generation: engine.gh_probe.generation,
            outcome: crate::gh::GhProbe::Decided {
                available: true,
                policy: crate::gh::GithubHostPolicy::Hosts(
                    ["github.com".to_string()].into_iter().collect(),
                ),
            },
        });
        assert!(engine.pr_sync.is_armed());

        engine.process_worker_event(crate::worker::WorkerEvent::GhStatusChecked {
            generation: engine.gh_probe.generation,
            outcome: crate::gh::GhProbe::Transient("timed out".to_string()),
        });

        assert!(
            engine.pr_sync.is_armed(),
            "a transient failure decides nothing and must not disarm",
        );
        assert_eq!(engine.pr_sync.poller_starts(), 1);
    }

    #[test]
    fn the_web_toggle_re_runs_the_host_probe_on_every_off_to_on_transition() {
        // A user who logs in to their enterprise host after starting the server
        // and then enables the integration must get a fresh answer from `gh`,
        // not the empty set the server booted with.
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program =
            crate::gh::probe_test_support::stand_in_gh(dir.path(), "exit 0").into();
        engine.github_integration_enabled = false;
        engine.config.ui.github_integration = false;
        assert_eq!(engine.gh_probe.generation, 0);

        engine
            .apply_wire(WireCommand::ToggleGithubIntegration {})
            .expect("toggle on");
        assert_eq!(engine.gh_probe.generation, 1, "enabling re-runs the probe");

        engine
            .apply_wire(WireCommand::ToggleGithubIntegration {})
            .expect("toggle off");
        assert_eq!(
            engine.gh_probe.generation, 1,
            "disabling asks gh nothing new",
        );

        engine
            .apply_wire(WireCommand::ToggleGithubIntegration {})
            .expect("toggle on again");
        assert_eq!(
            engine.gh_probe.generation, 2,
            "off and on again re-runs the probe a second time",
        );
    }

    /// The web app menu's "Re-check GitHub". It writes no config and it works
    /// while `gh` is broken, which is the only state it is for.
    #[test]
    fn apply_wire_recheck_github_asks_gh_again_and_recovers() {
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        engine.gh_probe.program =
            crate::gh::probe_test_support::stand_in_gh_serving(dir.path(), &["github.com"]).into();
        engine.github_integration_enabled = true;
        engine.config.ui.github_integration = true;
        engine.gh_status = crate::model::GhStatus::Unreachable;

        let outcome = engine
            .apply_wire(WireCommand::RecheckGithub {})
            .expect("apply re-check");
        assert!(
            outcome.status.is_some(),
            "the press is answered immediately, not silently",
        );
        assert_eq!(engine.gh_probe.generation, 1, "the probe was spawned");

        settle_gh_probe(&mut engine);
        assert_eq!(engine.gh_status, crate::model::GhStatus::Available);
        assert!(
            engine.config.ui.github_integration,
            "and nothing about the config was touched",
        );
    }

    #[test]
    fn apply_wire_toggle_github_integration_flips_flag() {
        let (mut engine, _tmp) = test_engine();
        let dir = tempfile::tempdir().expect("tempdir");
        // Enabling re-runs the host probe; point it at a stand-in serving one
        // host so the suite never shells out to the real `gh`.
        engine.gh_probe.program =
            crate::gh::probe_test_support::stand_in_gh_serving(dir.path(), &["github.com"]).into();
        engine.github_integration_enabled = false;
        engine.config.ui.github_integration = false;

        engine
            .apply_wire(WireCommand::ToggleGithubIntegration {})
            .expect("apply toggle");
        assert!(engine.github_integration_enabled, "runtime flag flips on");
        assert!(
            engine.config.ui.github_integration,
            "config flag flips on in lockstep with the runtime flag"
        );
        assert!(
            !engine.pr_sync.is_armed(),
            "the enable itself arms nothing: the probe it launched decides",
        );
        settle_gh_probe(&mut engine);
        assert!(
            engine.pr_sync.is_armed(),
            "the probe answering that a host works is what arms PR sync",
        );

        engine
            .apply_wire(WireCommand::ToggleGithubIntegration {})
            .expect("apply toggle back");
        assert!(!engine.github_integration_enabled);
        assert!(!engine.config.ui.github_integration);
        assert!(
            !engine.pr_sync.is_armed(),
            "disabling must disarm PR sync and clear cached PR statuses"
        );
        assert!(engine.pr_statuses.is_empty());
    }

    #[test]
    fn apply_wire_toggle_always_show_tab_strip_flips_and_persists() {
        let (mut engine, _tmp) = test_engine();
        let start = engine.config.ui.always_show_tab_strip;

        engine
            .apply_wire(WireCommand::ToggleAlwaysShowTabStrip {})
            .expect("apply toggle");
        assert_eq!(
            engine.config.ui.always_show_tab_strip, !start,
            "in-memory value must flip immediately"
        );

        engine.config_writer.flush();
        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains(&format!("always_show_tab_strip = {}", !start)),
            "flushed config must carry the flipped value, got:\n{disk}"
        );

        engine
            .apply_wire(WireCommand::ToggleAlwaysShowTabStrip {})
            .expect("apply toggle back");
        assert_eq!(engine.config.ui.always_show_tab_strip, start);
    }

    #[test]
    fn apply_wire_toggle_tab_reaches_agent_flips_and_persists() {
        let (mut engine, _tmp) = test_engine();
        let start = engine.config.ui.tab_reaches_agent;

        engine
            .apply_wire(WireCommand::ToggleTabReachesAgent {})
            .expect("apply toggle");
        assert_eq!(
            engine.config.ui.tab_reaches_agent, !start,
            "in-memory value must flip immediately"
        );

        engine.config_writer.flush();
        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains(&format!("tab_reaches_agent = {}", !start)),
            "flushed config must carry the flipped value, got:\n{disk}"
        );

        engine
            .apply_wire(WireCommand::ToggleTabReachesAgent {})
            .expect("apply toggle back");
        assert_eq!(engine.config.ui.tab_reaches_agent, start);
    }

    #[test]
    fn wire_kill_session_pty_deserializes() {
        let json = r#"{"command":"kill_session_pty","args":{"session_id":"s1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::KillSessionPty {
                session_id: "s1".to_string()
            }
        );
    }

    #[test]
    fn apply_wire_kill_session_pty_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::KillSessionPty {
                session_id: "ghost".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    /// The status names BOTH tabs the close moved: the one that went and the one
    /// that took its place, each the way the strip labelled it before the close.
    /// A toast that named only the successor left the user guessing which pill
    /// had just disappeared, and one built from the row provider could
    /// contradict the confirmation that raised it.
    #[test]
    fn apply_wire_close_slot_tab_names_the_closed_tab_and_its_successor() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = crate::model::AgentTab {
            id: "t2".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new("t2"), tab);

        let outcome = engine
            .apply_wire(WireCommand::CloseAgentTab {
                session_id: "s1".to_string(),
                tab_id: "s1-slot".to_string(),
            })
            .expect("apply close");

        assert_eq!(outcome.promoted.as_deref(), Some("t2"));
        assert_eq!(
            outcome.status.expect("a status").message,
            "Closed the first tab, Claude. Codex took its place as the agent's first tab."
        );
    }

    /// An extra tab's close names the tab that went, and says nothing about a
    /// successor, because the slot did not move.
    #[test]
    fn apply_wire_close_extra_tab_names_only_the_closed_tab() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = crate::model::AgentTab {
            id: "t2".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new("t2"), tab);

        let outcome = engine
            .apply_wire(WireCommand::CloseAgentTab {
                session_id: "s1".to_string(),
                tab_id: "t2".to_string(),
            })
            .expect("apply close");

        assert!(outcome.promoted.is_none());
        assert_eq!(
            outcome.status.expect("a status").message,
            "Closed the Codex tab."
        );
    }

    #[test]
    fn apply_wire_kill_session_pty_not_running_is_idempotent_noop() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let session = sample_session("s1", "p1", "feat");
        engine.sessions.push(session);

        // No provider in the map → nothing to kill, but it still succeeds.
        let outcome = engine
            .apply_wire(WireCommand::KillSessionPty {
                session_id: "s1".to_string(),
            })
            .expect("apply kill");
        let status = outcome.status.expect("a status");
        assert!(
            status.message.contains("is not running"),
            "msg: {}",
            status.message
        );
    }

    #[test]
    fn apply_wire_kill_session_pty_kills_and_detaches() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // Spawn a real `cat` PTY so the session counts as running.
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
        engine.mark_session_status("s1", crate::model::SessionStatus::Active);
        engine.mark_session_desired_running("s1", true);
        // Seed an auxiliary resume-state map the kill must also clear.
        engine
            .pty_input
            .insert("s1".to_string(), std::time::Instant::now());

        let outcome = engine
            .apply_wire(WireCommand::KillSessionPty {
                session_id: "s1".to_string(),
            })
            .expect("apply kill");
        let status = outcome.status.expect("a status");
        assert!(
            status.message.contains("Closed the last tab"),
            "msg: {}",
            status.message
        );
        // PTY removed from the live map and the session detached (not deleted).
        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "provider must be dropped"
        );
        assert!(
            engine.sessions.iter().any(|s| s.id == "s1"),
            "the session row must survive the kill"
        );
        assert_eq!(
            engine.sessions[0].status,
            crate::model::SessionStatus::Detached,
            "killed agent is detached, not deleted"
        );
        // The auxiliary resume state must be cleared so a later reconnect starts
        // clean, and desired_running cleared so startup auto-reopen won't relaunch
        // the agent the user explicitly killed.
        assert!(!engine.pty_input.contains_key("s1-slot"));
        assert!(
            !engine.sessions[0].desired_running,
            "an explicit kill clears desired_running"
        );
    }

    #[test]
    fn apply_wire_kill_session_pty_clears_in_flight_launch_key() {
        // kill_session_pty must route through `clear_tab_runtime`: a
        // hand-rolled tab-runtime clear
        // (providers/pins/pty_activity/pty_input/resume_fallback) misses the
        // in-flight `AgentLaunch` key, so a KillSessionPty racing an in-flight
        // launch leaves "already launching" reported forever.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

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
        engine.mark_session_status("s1", crate::model::SessionStatus::Active);
        engine.mark_in_flight(crate::engine::InFlightKey::AgentLaunch(TabId::new(
            "s1-slot",
        )));

        engine
            .apply_wire(WireCommand::KillSessionPty {
                session_id: "s1".to_string(),
            })
            .expect("apply kill");

        assert!(
            !engine.is_in_flight(&crate::engine::InFlightKey::AgentLaunch(TabId::new(
                "s1-slot"
            ))),
            "kill_session_pty must clear the in-flight AgentLaunch key for the killed tab"
        );
    }

    #[test]
    fn apply_wire_force_reconnect_clears_in_flight_launch_key() {
        // The force branch of `reconnect_session` must clear tab runtime state
        // through `clear_tab_runtime`, which also drops the in-flight
        // `AgentLaunch` key: a ForceReconnect racing a stale in-flight launch
        // for that tab would otherwise leave the key set, and the
        // `DispatchAgentLaunch` chokepoint would refuse the new launch with
        // "already launching".
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        let wt = tmp.path().join("wt-s1");
        std::fs::create_dir_all(&wt).unwrap();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = wt.to_string_lossy().to_string();
        engine.sessions.push(session);
        // Simulate a stale in-flight marker left behind by a prior launch.
        engine.mark_in_flight(crate::engine::InFlightKey::AgentLaunch(TabId::new("s1")));

        let outcome = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: true,
            })
            .expect("apply force reconnect");

        let status = outcome
            .status
            .expect("force reconnect surfaces a busy status");
        assert_eq!(
            status.tone, "busy",
            "the stale in-flight key must be cleared before dispatch so the new \
             launch proceeds instead of being refused as \"already launching\": {}",
            status.message
        );
        assert!(
            status.message.contains("Starting fresh agent \"s1-title\""),
            "msg: {}",
            status.message
        );
    }

    #[test]
    fn apply_wire_detach_agent_stops_every_tab() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // An extra tab with its own row, plus a live provider for BOTH the
        // session-slot tab (keyed by its own tab id) and the extra tab.
        let tab = crate::model::AgentTab {
            id: "tab-2".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

        let spawn = |name: &str| {
            crate::pty::PtyClient::spawn_with_env(
                name,
                &[],
                worktree.path(),
                24,
                80,
                engine.config.ui.agent_scrollback_lines,
                &[],
            )
            .expect("spawn provider")
        };
        engine.providers.insert(TabId::new("s1-slot"), spawn("cat"));
        engine.providers.insert(TabId::new("tab-2"), spawn("cat"));
        engine.mark_session_status("s1", crate::model::SessionStatus::Active);
        engine.mark_session_desired_running("s1", true);

        let outcome = engine
            .apply_wire(WireCommand::DetachAgent {
                session_id: "s1".to_string(),
            })
            .expect("apply detach");
        let status = outcome.status.expect("a status");
        assert!(
            status.message.contains("Detached agent") && status.message.contains("2 tab"),
            "msg: {}",
            status.message
        );
        // EVERY tab's provider is gone — not just the session-slot one.
        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "session-slot tab stopped"
        );
        assert!(
            !engine.providers.contains_key(TabIdRef::new("tab-2")),
            "extra tab stopped"
        );
        // The agent is detached (not deleted) and its rows survive.
        assert_eq!(
            engine.sessions[0].status,
            crate::model::SessionStatus::Detached
        );
        assert!(
            engine.agent_tabs.contains_key(TabIdRef::new("tab-2")),
            "detach keeps tab rows (reopenable)"
        );
        assert!(
            !engine.sessions[0].desired_running,
            "detach clears desired_running"
        );
    }

    #[test]
    fn wire_checkout_project_default_branch_deserializes() {
        let json = r#"{"command":"checkout_project_default_branch","args":{"project_id":"p1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            }
        );
    }

    // Initialize a repo on `default_branch` with one commit, then create and
    // check out `feature` so HEAD sits off the default. Returns the temp dir.
    fn init_repo_on_feature_branch(default_branch: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", default_branch]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        run(&["commit", "--allow-empty", "-q", "-m", "init"]);
        run(&["switch", "-q", "-c", "feature"]);
        dir
    }

    fn current_git_branch(repo: &std::path::Path) -> String {
        let out = crate::git::test_support::git_command()
            .args([
                "-C",
                repo.to_string_lossy().as_ref(),
                "symbolic-ref",
                "--short",
                "HEAD",
            ])
            .output()
            .expect("spawn git");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // Drive the checkout-default-branch worker chain the same way the web engine
    // actor's drain does (engine_actor.rs): pull the inspection event (worker 1)
    // off `worker_rx`, run `process_worker_event`, then feed the reaction through
    // BOTH `wire_statuses_from_reaction` (the Status outcomes) and
    // `drive_checkout_followup` (the Known case, which spawns worker 2). Returns
    // every status the actor would broadcast across both phases, in order.
    fn drive_checkout_chain(engine: &mut Engine) -> Vec<WireStatus> {
        let mut statuses = Vec::new();
        // Phase 1: the inspection worker. The Known case spawns worker 2; the
        // other cases produce their final status here.
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("inspection worker event");
        let reaction = engine.process_worker_event(event);
        statuses.extend(wire_statuses_from_reaction(&reaction));
        let spawned_switch = matches!(
            reaction,
            EventReaction::DispatchProjectDefaultBranchCheckout { .. }
        );
        statuses.extend(engine.drive_checkout_followup(&reaction));
        if spawned_switch {
            // Phase 2: the switch worker's completion carries the success/failure
            // status, surfaced by the actor's wire_statuses_from_reaction drain.
            let event = engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("switch worker event");
            let reaction = engine.process_worker_event(event);
            statuses.extend(wire_statuses_from_reaction(&reaction));
        }
        statuses
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_switches_from_feature() {
        // A project whose persisted leading branch ("trunk") differs from HEAD
        // ("feature") gets switched back, and the in-memory state updates to
        // Leading — mirroring the TUI's Known-default-branch outcome. apply_wire
        // now returns Busy and spawns the inspection worker; the switch happens
        // off-thread, driven through the two-worker chain like the web actor.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        project.leading_branch = Some("trunk".to_string());
        project.current_branch = "feature".to_string();
        project.branch_status = ProjectBranchStatus::NotLeading;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        let busy = outcome.status.expect("busy status");
        assert_eq!(busy.tone, "busy");
        assert!(
            busy.message
                .contains("Checking the default branch for project \"p1-name\""),
            "unexpected busy message: {}",
            busy.message
        );

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "info");
        assert!(
            status
                .message
                .contains("Checked out \"trunk\" for project \"p1-name\""),
            "unexpected message: {}",
            status.message
        );
        // HEAD actually moved in the temp repo.
        assert_eq!(current_git_branch(repo.path()), "trunk");
        let updated = &engine.projects[0];
        assert_eq!(updated.current_branch, "trunk");
        assert_eq!(updated.branch_status, ProjectBranchStatus::Leading);
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_switch_failure_reports_error() {
        // Worker 2 fails: the persisted leading branch ("ghost") does not exist,
        // so `git switch` errors. The inspection (which trusts the persisted
        // leading branch) yields Known, worker 2 runs the switch and fails, and
        // the completion arm surfaces the TUI's exact failure wording. HEAD stays
        // on feature.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        project.leading_branch = Some("ghost".to_string());
        project.current_branch = "feature".to_string();
        project.branch_status = ProjectBranchStatus::NotLeading;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert!(
            status.message.contains("Couldn't check out \"ghost\" in")
                && status
                    .message
                    .contains("resolve in your terminal and retry"),
            "unexpected message: {}",
            status.message
        );
        // The failed switch leaves HEAD where it was.
        assert_eq!(current_git_branch(repo.path()), "feature");
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_inspection_error_reports_error() {
        // Worker 1 fails: the project's path points at a directory that is not a
        // git repo, so `git::current_branch` errors. (The `path_missing` guard is
        // a stored flag, not a live filesystem check, so a non-repo path with
        // `path_missing == false` reaches the inspection worker.) The inspection
        // arm surfaces the "Couldn't inspect the default branch" wording and no
        // switch is spawned.
        let not_a_repo = tempfile::tempdir().expect("tempdir");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", not_a_repo.path().to_string_lossy().as_ref());
        project.leading_branch = None;
        project.current_branch = "feature".to_string();
        project.path_missing = false;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert!(
            status
                .message
                .contains("Couldn't inspect the default branch for project \"p1-name\""),
            "unexpected message: {}",
            status.message
        );
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_already_on_leading_is_noop() {
        // HEAD already equals the persisted leading branch: no switch, an info
        // status, and the branch is left untouched (the TUI's None outcome).
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        // Pin the leading branch to the branch HEAD is on so it is already leading.
        project.leading_branch = Some("feature".to_string());
        project.current_branch = "feature".to_string();
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "info");
        assert!(
            status
                .message
                .contains("already on the leading branch \"feature\""),
            "unexpected message: {}",
            status.message
        );
        assert_eq!(current_git_branch(repo.path()), "feature");
        assert_eq!(
            engine.projects[0].branch_status,
            ProjectBranchStatus::Leading
        );
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_heuristic_errors() {
        // No origin/HEAD and HEAD is neither main nor master, so the default
        // branch can't be determined: the TUI refuses with an error status
        // rather than guessing. No leading_branch is persisted here so the
        // heuristic path runs.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        project.leading_branch = None;
        project.current_branch = "feature".to_string();
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert!(
            status
                .message
                .contains("Can't determine the default branch"),
            "unexpected message: {}",
            status.message
        );
        // Heuristic refusal leaves HEAD where it was.
        assert_eq!(current_git_branch(repo.path()), "feature");
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_unknown_project_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "ghost".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_path_missing_errors() {
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/repo");
        project.path_missing = true;
        engine.projects.push(project);
        let err = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot check out default branch: path not found for \"p1-name\""),
            "err: {err}"
        );
    }

    #[test]
    fn checkout_default_branch_busy_is_keyed() {
        // `sample_project` has path_missing=false, so the busy is returned
        // synchronously (the inspection worker is spawned in the background and
        // its result is irrelevant to this test).
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".into(),
            })
            .expect("apply");
        let s = outcome.status.expect("busy status");
        assert_eq!(s.tone, "busy");
        // The busy now carries the checkout op's opaque correlation id (not a
        // hand-authored key); its final shares the same id so it replaces this
        // spinner. The op is registered until the worker chain resolves it.
        let key = s.key.as_deref().expect("checkout busy must be keyed");
        assert!(
            key.starts_with("op-"),
            "checkout busy must carry the StatusOp's opaque id, got {key:?}"
        );
        assert!(
            engine.pending_web_checkout_ops.contains_key(key),
            "the checkout op must be registered under its busy key"
        );
    }

    #[test]
    fn checkout_default_branch_resolves_op_with_keyed_success() {
        // Drive the full two-worker chain (inspect → switch) and assert the
        // success final REPLACES the busy on the same opaque op id, with the
        // byte-identical message — the migration's core invariant.
        let (_origin, _clone, work) = clone_repo_on_feature_branch("main");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", &work.to_string_lossy());
        // Force the Known-default path: no stored leading branch, so worker 1
        // resolves the default branch from the clone's origin/HEAD.
        project.leading_branch = None;
        project.path_missing = false;
        engine.projects.push(project);

        let busy = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".into(),
            })
            .expect("apply")
            .status
            .expect("busy");
        let busy_key = busy.key.clone().expect("checkout busy must be keyed");

        // Worker 1 (inspection) → DispatchProjectDefaultBranchCheckout reaction →
        // the followup spawns worker 2 (the switch).
        let ev1 = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("inspection event");
        let r1 = engine.process_worker_event(ev1);
        let _ = engine.drive_checkout_followup(&r1);

        // Worker 2 (switch) completion resolves the op into the keyed final.
        let ev2 = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("switch event");
        let r2 = engine.process_worker_event(ev2);
        let statuses = wire_statuses_from_reaction(&r2);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "info");
        assert_eq!(
            status.message,
            "Checked out \"main\" for project \"p1-name\"."
        );
        assert_eq!(
            status.key.as_deref(),
            Some(busy_key.as_str()),
            "the checkout final must reuse the busy's op id"
        );
        assert!(
            !engine.pending_web_checkout_ops.contains_key(&busy_key),
            "the checkout op must be consumed once resolved"
        );
        assert_eq!(current_git_branch(&work), "main");
    }

    #[test]
    fn create_agent_surfaces_created_op_id_for_correlation() {
        // A synchronous create dispatch returns its create op id in the outcome so
        // a REST handler can correlate ITS exact new session race-free (rather than
        // a set-difference that could pick a concurrent create's session). The op
        // id is also the key under which the create op is registered.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        let project = sample_project("p1", &repo.path().to_string_lossy());
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "my-agent".to_string(),
                copy_uncommitted_changes: None,
                use_existing_branch: false,
            })
            .expect("dispatch create");

        let op_id = outcome
            .created_op_id
            .expect("a synchronous create must surface its op id");
        assert!(op_id.starts_with("op-"), "got {op_id:?}");
        assert!(
            engine.pending_create_ops.contains_key(&op_id),
            "the create op must be registered under the surfaced id"
        );

        // A non-create command surfaces no create op id, so the handler never
        // mistakes a plain command's outcome for a create to correlate.
        let other = engine
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: true })
            .expect("toggle changes pane");
        assert_eq!(other.created_op_id, None);
    }

    #[test]
    fn attach_pull_request_pins_and_clear_override_unpins() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("persist session");
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::AttachPullRequest {
                session_id: "s1".to_string(),
                host: "GitHub.com".to_string(),
                owner_repo: "fork/r".to_string(),
                number: 12,
                title: "Pinned".to_string(),
                state: "OPEN".to_string(),
                url: "https://github.com/fork/r/pull/12".to_string(),
            })
            .expect("attach");
        let status = outcome.status.expect("attach confirmation");
        assert_eq!(status.tone, "info");
        assert!(status.message.contains("#12"), "got {:?}", status.message);

        // The pin is persisted, mirrored in memory, and drives the badge.
        let stored = engine.session_store.load_pr_overrides().expect("load");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].pr_number, 12);
        assert_eq!(
            stored[0].host, "github.com",
            "hosts are lowercased at the boundary"
        );
        assert_eq!(engine.pr_overrides.get("s1").map(|p| p.pr_number), Some(12));
        assert_eq!(engine.pr_statuses.get("s1").map(|p| p.number), Some(12));
        // The sync snapshot now carries the pin for the worker.
        let entries = engine.pr_sync_sessions.lock().unwrap().clone();
        let pin = entries[0].pinned.as_ref().expect("pinned sync entry");
        assert_eq!(pin.number, 12);
        assert_eq!(pin.owner_repo, "fork/r");

        // Detach: the row and the map entry go, the badge goes WITH them (a
        // detach means "no pull request", now, not at the next cycle), and the
        // session drops out of the sync plan entirely.
        let outcome = engine
            .apply_wire(WireCommand::ClearPullRequestOverride {
                session_id: "s1".to_string(),
            })
            .expect("detach");
        assert_eq!(outcome.status.expect("detach confirmation").tone, "info");
        assert!(engine.session_store.load_pr_overrides().unwrap().is_empty());
        assert!(engine.pr_overrides.is_empty());
        assert!(
            !engine.pr_statuses.contains_key("s1"),
            "the badge disappears with the detach"
        );
        assert!(
            engine.pr_sync_sessions.lock().unwrap().is_empty(),
            "a detached session is not planned for at all"
        );

        // Resume: the session rejoins the plan, and the confirmation says so.
        let outcome = engine
            .apply_wire(WireCommand::ResumePullRequestAutodetection {
                session_id: "s1".to_string(),
            })
            .expect("resume");
        let status = outcome.status.expect("resume confirmation");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("Resumed"),
            "got {:?}",
            status.message
        );
        assert!(engine.pr_suppressions.is_empty());
        assert_eq!(engine.pr_sync_sessions.lock().unwrap().len(), 1);
    }

    /// Both PR-detachment commands refuse a session that is not there, so the
    /// REST layer can map the message onto a 404 the way its siblings do.
    #[test]
    fn detach_and_resume_refuse_an_unknown_session() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        for command in [
            WireCommand::ClearPullRequestOverride {
                session_id: "ghost".to_string(),
            },
            WireCommand::ResumePullRequestAutodetection {
                session_id: "ghost".to_string(),
            },
        ] {
            let err = engine
                .apply_wire(command)
                .expect_err("a vanished session must refuse");
            assert!(err.to_string().contains("unknown session"), "got {err:#}");
        }
    }

    /// The stored pin's host must be byte-identical to what the sync planner
    /// derives: empty means github.com and everything is lowercased. A raw
    /// wire command can carry any spelling, and an unnormalized stored host
    /// would never match the planner's query target, so the pin would never
    /// refresh.
    #[test]
    fn attach_pull_request_normalizes_the_stored_host_like_the_planner() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine
            .apply_pr_attach("s1", "", "o/r", 1, "", "OPEN", "")
            .expect("attach with an empty host");
        assert_eq!(
            engine.pr_overrides.get("s1").map(|p| p.host.as_str()),
            Some("github.com"),
            "an empty host stores as github.com"
        );

        engine
            .apply_pr_attach("s1", "GitHub.Example.COM ".trim(), "o/r", 2, "", "OPEN", "")
            .expect("attach with a capitalised host");
        assert_eq!(
            engine.pr_overrides.get("s1").map(|p| p.host.as_str()),
            Some("github.example.com"),
            "hosts store lowercased"
        );
    }

    #[test]
    fn attach_pull_request_refuses_an_unknown_session_and_an_unknown_state() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        let err = engine
            .apply_wire(WireCommand::AttachPullRequest {
                session_id: "ghost".to_string(),
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                number: 1,
                title: String::new(),
                state: "OPEN".to_string(),
                url: String::new(),
            })
            .expect_err("a vanished session must refuse the attach");
        assert!(err.to_string().contains("ghost"), "got {err:#}");

        let session = sample_session("s1", "p1", "feat");
        engine
            .session_store
            .upsert_session(&session)
            .expect("persist session");
        engine.sessions.push(session);
        let err = engine
            .apply_wire(WireCommand::AttachPullRequest {
                session_id: "s1".to_string(),
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                number: 1,
                title: String::new(),
                state: "DRAFT".to_string(),
                url: String::new(),
            })
            .expect_err("only OPEN/MERGED/CLOSED are storable states");
        assert!(err.to_string().contains("DRAFT"), "got {err:#}");
        assert!(engine.pr_overrides.is_empty());
    }

    #[test]
    fn dispatch_attach_pull_request_mints_one_keyed_op_spanning_resolve_and_attach() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let (op_id, pending) = engine
            .dispatch_attach_pull_request("s1", "#42")
            .expect("dispatch");
        assert_eq!(pending.tone, StatusTone::Busy);
        assert_eq!(pending.key.as_deref(), Some(op_id.as_str()));
        assert!(engine.pending_pr_attach_ops.contains_key(&op_id));

        // A second attach for the same session while the first is still
        // resolving is refused as busy, not queued behind it.
        let err = engine
            .dispatch_attach_pull_request("s1", "#43")
            .expect_err("one attach at a time per agent");
        assert!(
            err.to_string()
                .contains(crate::engine::PR_ATTACH_IN_FLIGHT_MARKER),
            "got {err:#}",
        );

        // Gated exactly like the new-agent-from-PR flow. Resolve the pending
        // attach first so this asserts the gh gate rather than the busy guard.
        engine.clear_in_flight(&crate::engine::InFlightKey::PrAttach("s1".to_string()));
        engine.gh_status = crate::model::GhStatus::NotInstalled;
        let err = engine
            .dispatch_attach_pull_request("s1", "#42")
            .expect_err("no gh, no attach");
        assert!(err.to_string().contains("gh CLI"), "got {err:#}");
    }

    /// The headline of the mutual-blocking ruling: while an attach is
    /// resolving, a detach for the same agent is refused outright with an
    /// honest message instead of racing the attach's own write.
    #[test]
    fn a_pending_attach_blocks_detach_and_resume_for_the_same_session() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let other = sample_session("s2", "p1", "other");
        engine.session_store.upsert_session(&other).unwrap();
        engine.sessions.push(other);

        engine
            .dispatch_attach_pull_request("s1", "#42")
            .expect("dispatch");
        assert!(engine.is_in_flight(&crate::engine::InFlightKey::PrAttach("s1".to_string())));

        let err = engine
            .clear_pull_request_override("s1")
            .expect_err("a detach must not race a resolving attach");
        assert!(
            err.to_string()
                .contains(crate::engine::PR_ATTACH_IN_FLIGHT_MARKER),
            "got {err:#}",
        );
        assert!(
            !engine.pr_suppressions.contains("s1"),
            "the refused detach must not have written the suppression"
        );

        let err = engine
            .resume_pr_autodetection("s1")
            .expect_err("a resume must not race a resolving attach either");
        assert!(
            err.to_string()
                .contains(crate::engine::PR_ATTACH_IN_FLIGHT_MARKER),
            "got {err:#}",
        );

        // The block is per session: another agent's PR operations are untouched.
        engine
            .clear_pull_request_override("s2")
            .expect("an unrelated agent still detaches");
    }

    /// Every attach dispatch ends in exactly one `PullRequestResolved`, and
    /// that arm is what unblocks the session. Both outcomes clear it: a
    /// successful attach and a failed lookup.
    #[test]
    fn an_attach_resolution_unblocks_the_session_on_success_and_on_failure() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let (op_id, _) = engine
            .dispatch_attach_pull_request("s1", "#12")
            .expect("dispatch");
        engine.process_worker_event(crate::worker::WorkerEvent::PullRequestResolved {
            result: Ok(crate::worker::ResolvedPullRequest {
                project: sample_project("p1", "/tmp/p1"),
                host: "github.com".to_string(),
                owner_repo: "o/r".to_string(),
                number: 12,
                title: "Pinned".to_string(),
                state: "OPEN".to_string(),
                head_ref_name: "feat".to_string(),
                custom_name: None,
            }),
            purpose: crate::worker::PrLookupPurpose::Attach {
                session_id: "s1".to_string(),
            },
            status_op_id: Some(op_id),
        });
        assert!(
            !engine.is_in_flight(&crate::engine::InFlightKey::PrAttach("s1".to_string())),
            "a landed attach unblocks the session"
        );
        engine
            .clear_pull_request_override("s1")
            .expect("detach works again once the attach has landed");
        engine
            .resume_pr_autodetection("s1")
            .expect("and so does resume");

        // The failure path: a lookup that never found a PR must unblock too,
        // and the user's very next act is typically the detach they were
        // refused, so that interleaving is exercised directly.
        let (op_id, _) = engine
            .dispatch_attach_pull_request("s1", "#99")
            .expect("dispatch");
        engine.process_worker_event(crate::worker::WorkerEvent::PullRequestResolved {
            result: Err("no such pull request".to_string()),
            purpose: crate::worker::PrLookupPurpose::Attach {
                session_id: "s1".to_string(),
            },
            status_op_id: Some(op_id),
        });
        assert!(
            !engine.is_in_flight(&crate::engine::InFlightKey::PrAttach("s1".to_string())),
            "a failed attach unblocks the session as surely as a successful one"
        );
        engine
            .clear_pull_request_override("s1")
            .expect("detach immediately after a failed attach");
    }

    /// The fallback resolution path (no keyed op id, or an op id the map no
    /// longer holds) still has to unblock the session: the clear is keyed on
    /// the purpose's session id, never on the op id.
    #[test]
    fn an_attach_resolution_without_its_keyed_op_still_unblocks_the_session() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let (op_id, _) = engine
            .dispatch_attach_pull_request("s1", "#12")
            .expect("dispatch");
        engine.pending_pr_attach_ops.remove(&op_id);
        engine.process_worker_event(crate::worker::WorkerEvent::PullRequestResolved {
            result: Err("no such pull request".to_string()),
            purpose: crate::worker::PrLookupPurpose::Attach {
                session_id: "s1".to_string(),
            },
            status_op_id: None,
        });
        assert!(
            !engine.is_in_flight(&crate::engine::InFlightKey::PrAttach("s1".to_string())),
            "the op-id-missing fallback clears the guard too"
        );
    }

    /// The engine-side attach arm: a resolved lookup applies the pin and the
    /// SAME keyed op carries the final; a session deleted mid-lookup attaches
    /// nothing (the real vanished-session guard for both surfaces).
    #[test]
    fn pr_attach_resolution_applies_the_pin_or_refuses_a_vanished_session() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let (op_id, _) = engine
            .dispatch_attach_pull_request("s1", "#12")
            .expect("dispatch");

        let resolved = crate::worker::ResolvedPullRequest {
            project: sample_project("p1", "/tmp/p1"),
            host: "github.com".to_string(),
            owner_repo: "forker/Hello-World".to_string(),
            number: 12,
            title: "Pinned".to_string(),
            state: "OPEN".to_string(),
            head_ref_name: "feat".to_string(),
            custom_name: None,
        };
        let reaction =
            engine.process_worker_event(crate::worker::WorkerEvent::PullRequestResolved {
                result: Ok(resolved.clone()),
                purpose: crate::worker::PrLookupPurpose::Attach {
                    session_id: "s1".to_string(),
                },
                status_op_id: Some(op_id.clone()),
            });
        let statuses = wire_statuses_from_reaction(&reaction);
        let final_status = statuses.last().expect("keyed final");
        assert_eq!(final_status.tone, "info");
        assert_eq!(final_status.key.as_deref(), Some(op_id.as_str()));
        assert!(final_status.message.contains("#12"), "{final_status:?}");
        assert!(!engine.pending_pr_attach_ops.contains_key(&op_id));
        assert_eq!(engine.pr_overrides.get("s1").map(|p| p.pr_number), Some(12));
        assert_eq!(engine.pr_statuses.get("s1").map(|p| p.number), Some(12));
        assert_eq!(engine.session_store.load_pr_overrides().unwrap().len(), 1);

        // Session deleted mid-lookup: nothing is attached, the keyed final is
        // an error, and no orphan row is written for the dead id.
        engine.pr_overrides.clear();
        engine.pr_statuses.clear();
        engine.session_store.delete_pr_override("s1").unwrap();
        let (op_id, _) = engine
            .dispatch_attach_pull_request("s1", "#12")
            .expect("dispatch");
        engine.sessions.clear();
        let reaction =
            engine.process_worker_event(crate::worker::WorkerEvent::PullRequestResolved {
                result: Ok(resolved),
                purpose: crate::worker::PrLookupPurpose::Attach {
                    session_id: "s1".to_string(),
                },
                status_op_id: Some(op_id.clone()),
            });
        let statuses = wire_statuses_from_reaction(&reaction);
        let final_status = statuses.last().expect("keyed final");
        assert_eq!(final_status.tone, "error");
        assert!(final_status.message.contains("deleted"), "{final_status:?}");
        assert!(engine.pr_overrides.is_empty());
        assert!(
            engine.session_store.load_pr_overrides().unwrap().is_empty(),
            "no override row is written for a dead session id"
        );
    }

    #[test]
    fn create_agent_from_pr_busy_is_keyed() {
        // When gh is available and the project exists, the synchronous busy
        // returned by `CreateAgentFromPr` must carry `pr-lookup:{project_id}`.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        let mut project = sample_project("p1", &repo.path().to_string_lossy());
        project.path_missing = false;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "#42".to_string(),
                name: "my-agent".to_string(),
            })
            .expect("dispatch lookup");
        let status = outcome.status.expect("synchronous busy status");
        assert_eq!(status.tone, "busy");
        // The busy now carries the PR-lookup op's opaque correlation id; on
        // success the followup clears it (the create busy takes over), on failure
        // it is resolved to a keyed error.
        let key = status.key.as_deref().expect("PR busy must be keyed");
        assert!(
            key.starts_with("op-"),
            "PR busy must carry the StatusOp's opaque id, got {key:?}"
        );
        assert!(
            engine.pending_web_pr_lookup_ops.contains_key(key),
            "the PR-lookup op must be registered under its busy key"
        );
    }

    #[test]
    fn pr_lookup_failure_resolves_op_with_keyed_error() {
        // Closes the previously-documented gap: a PR-lookup FAILURE used to leave
        // its busy stranded (the failure event carried no project id). Now the
        // failure resolves the op into a keyed error that REPLACES the busy.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        let mut project = sample_project("p1", &repo.path().to_string_lossy());
        project.path_missing = false;
        engine.projects.push(project);

        let busy_key = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "#42".to_string(),
                name: "my-agent".to_string(),
            })
            .expect("dispatch")
            .status
            .expect("busy")
            .key
            .expect("keyed busy");

        // Simulate the lookup worker reporting a failure carrying the op id (the
        // real worker forwards the id it was spawned with).
        let reaction =
            engine.process_worker_event(crate::worker::WorkerEvent::PullRequestResolved {
                result: Err("gh pr view failed".to_string()),
                purpose: crate::worker::PrLookupPurpose::CreateAgent,
                status_op_id: Some(busy_key.clone()),
            });
        let statuses = wire_statuses_from_reaction(&reaction);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert_eq!(status.message, "gh pr view failed");
        assert_eq!(
            status.key.as_deref(),
            Some(busy_key.as_str()),
            "the PR-lookup failure must reuse the busy's op id to replace it"
        );
        assert!(
            !engine.pending_web_pr_lookup_ops.contains_key(&busy_key),
            "the PR-lookup op must be consumed once resolved"
        );
    }

    // Clone `origin` (init'd on `default_branch`) into a fresh working tree and
    // check out `feature/x`, so HEAD sits off the KNOWN default — `git clone`
    // sets refs/remotes/origin/HEAD, so `branch_warning_kind` resolves Known.
    // Returns (origin tempdir, clone tempdir, clone working-tree path).
    fn clone_repo_on_feature_branch(
        default_branch: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
        let origin = init_repo_on_feature_branch(default_branch);
        // The helper leaves origin on `feature`; put it back on the default so
        // the clone's origin/HEAD points there.
        let run_in = |dir: &std::path::Path, args: &[&str]| {
            let ok = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir)
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run_in(origin.path(), &["switch", "-q", default_branch]);

        let clone_dir = tempfile::tempdir().expect("clone tempdir");
        let work = clone_dir.path().join("work");
        run_in(
            clone_dir.path(),
            &[
                "clone",
                "-q",
                origin.path().to_string_lossy().as_ref(),
                work.to_string_lossy().as_ref(),
            ],
        );
        run_in(&work, &["switch", "-q", "-c", "feature/x"]);
        (origin, clone_dir, work)
    }

    // Drive the add-project "Check Out & Add" worker chain the way the web actor
    // does: pull worker 2's switch-completion off `worker_rx`, run
    // `process_worker_event`, feed the reaction through both
    // `wire_statuses_from_reaction` (switch FAILURE error) and
    // `drive_add_project_followup` (switch SUCCESS → inline persistence add).
    // The Add path is now inline so no second worker event is drained.
    fn drive_add_project_chain(engine: &mut Engine) -> Vec<WireStatus> {
        let mut statuses = Vec::new();
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("switch worker event");
        let reaction = engine.process_worker_event(event);
        statuses.extend(wire_statuses_from_reaction(&reaction));
        statuses.extend(engine.drive_add_project_followup(&reaction));
        statuses
    }

    #[test]
    fn wire_add_project_checkout_default_deserializes() {
        let json = r#"{"command":"add_project_checkout_default","args":{"path":"/repo","name":"My Project"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::AddProjectCheckoutDefault {
                path: "/repo".to_string(),
                name: "My Project".to_string(),
            }
        );
    }

    /// A tempdir repo initialized with `git init` but no commit (unborn HEAD).
    fn init_unborn_repo_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            assert!(
                crate::git::test_support::git_command()
                    .args(args)
                    .current_dir(dir.path())
                    .status()
                    .expect("spawn git")
                    .success(),
                "git {args:?} failed"
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "test"]);
        dir
    }

    #[test]
    fn apply_wire_create_initial_commit_bootstraps_then_adds() {
        let repo = init_unborn_repo_dir();
        let path = repo.path().to_string_lossy().into_owned();
        let (mut engine, _tmp) = test_engine();

        engine
            .apply_wire(WireCommand::AddProjectCreateInitialCommit {
                path: path.clone(),
                name: "Demo".to_string(),
            })
            .expect("dispatch initial-commit add");
        // Drive the worker chain (InitialCommitCreated → AddProjectAfterInitialCommit
        // → inline registration) exactly like the checkout-default chain.
        let statuses = drive_add_project_chain(&mut engine);

        assert!(
            crate::git::repo_has_commits(repo.path()),
            "the repo must have a commit after the chain completes"
        );
        assert_eq!(engine.projects.len(), 1, "the project must be registered");
        assert_eq!(engine.projects[0].current_branch, "main");
        assert!(
            statuses.iter().any(|s| s.tone == "info"),
            "a success status must be emitted, got {statuses:?}"
        );
    }

    #[test]
    fn apply_wire_create_initial_commit_rejects_a_concurrent_request() {
        let repo = init_unborn_repo_dir();
        // The in-flight gate is keyed by the CANONICAL path (validation
        // canonicalizes), so pre-mark with the same.
        let canonical = std::fs::canonicalize(repo.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (mut engine, _tmp) = test_engine();
        assert!(
            engine.mark_in_flight(crate::engine::InFlightKey::InitialCommit(canonical)),
            "precondition: gate starts free"
        );

        let res = engine.apply_wire(WireCommand::AddProjectCreateInitialCommit {
            path: repo.path().to_string_lossy().into_owned(),
            name: "Demo".to_string(),
        });
        assert!(
            res.is_err(),
            "a second concurrent initial-commit request for the same repo must be rejected"
        );
        assert!(
            !crate::git::repo_has_commits(repo.path()),
            "the rejected request must not have committed"
        );
    }

    #[test]
    fn apply_wire_init_repo_shares_the_initial_commit_gate() {
        // Catches double-bootstrap races: a second concurrent AddProjectInitRepo
        // for the same path is rejected, and an AddProjectCreateInitialCommit
        // for a path an init flow holds is likewise excluded (shared key).
        let plain = tempfile::tempdir().expect("tempdir");
        let path = plain.path().to_string_lossy().into_owned();
        let (mut engine, _tmp) = test_engine();

        engine
            .apply_wire(WireCommand::AddProjectInitRepo {
                path: path.clone(),
                name: "Adopted".to_string(),
            })
            .expect("first init-repo dispatch");
        let res = engine.apply_wire(WireCommand::AddProjectInitRepo {
            path: path.clone(),
            name: "Adopted".to_string(),
        });
        assert!(
            res.is_err(),
            "a second concurrent init-repo request for the same path must be rejected"
        );

        // Cross-exclusion with the commit-only flow: an unborn repo whose path
        // is held by the shared key can't start a commit-only bootstrap.
        let unborn = init_unborn_repo_dir();
        let canonical = std::fs::canonicalize(unborn.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(engine.mark_in_flight(crate::engine::InFlightKey::InitialCommit(canonical)));
        let res = engine.apply_wire(WireCommand::AddProjectCreateInitialCommit {
            path: unborn.path().to_string_lossy().into_owned(),
            name: "Demo".to_string(),
        });
        assert!(
            res.is_err(),
            "the commit-only flow must be excluded by the shared in-flight key"
        );
        // Drain the first dispatch's worker event so its thread doesn't leak
        // an un-received send into other tests' channels.
        let _ = drive_add_project_chain(&mut engine);
    }

    #[test]
    fn apply_wire_init_repo_clears_the_gate_and_resolves_the_op() {
        // Catches a stranded busy toast plus a permanently locked path.
        // Asserts only outcome-independent invariants (the commit outcome
        // depends on whether this machine has an ambient git identity).
        let plain = tempfile::tempdir().expect("tempdir");
        let path = plain.path().to_string_lossy().into_owned();
        let (mut engine, _tmp) = test_engine();

        let outcome = engine
            .apply_wire(WireCommand::AddProjectInitRepo {
                path: path.clone(),
                name: "Adopted".to_string(),
            })
            .expect("dispatch init-repo add");
        let busy = outcome.status.expect("a keyed busy status");
        assert_eq!(busy.tone, "busy");

        let statuses = drive_add_project_chain(&mut engine);
        assert!(
            !statuses.is_empty(),
            "the busy must resolve to a final status either way"
        );
        assert!(
            engine.pending_web_add_project_ops.is_empty(),
            "the keyed op must be consumed, not stranded"
        );
        // The gate must be free again whatever the outcome was.
        let canonical = std::fs::canonicalize(plain.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            engine.mark_in_flight(crate::engine::InFlightKey::InitialCommit(canonical)),
            "the in-flight gate must be cleared after the worker completes"
        );
    }

    #[test]
    fn seed_warning_survives_a_commit_failure() {
        // Catches the double-failure swallow: when the seed failed AND the
        // commit failed, the error's recovery advice routes the user through
        // the commit-only rung (which never seeds), so the seed warning must
        // still surface as its own persistent warning alongside the error
        // final; dropping it recreates the silent-seedless outcome it exists
        // to prevent.
        let (mut engine, _tmp) = test_engine();
        let reaction =
            engine.process_worker_event(crate::worker::WorkerEvent::InitialCommitCreated {
                add: crate::worker::InitialCommitAdd {
                    path: "/adopted".to_string(),
                    name: "Adopted".to_string(),
                    branch: String::new(),
                    leading_branch: String::new(),
                    initialized_repo: true,
                    seeded_gitignore: false,
                    seed_warning: Some("seed failed".to_string()),
                },
                result: Err("no identity".to_string()),
                status_op_id: None,
            });
        let statuses = wire_statuses_from_reaction(&reaction);
        assert!(
            statuses
                .iter()
                .any(|s| s.tone == "warning" && s.message == "seed failed"),
            "the seed warning must survive the commit failure, got {statuses:?}"
        );
        assert!(
            statuses
                .iter()
                .any(|s| s.tone == "error" && s.message.contains("no identity")),
            "the error final must still be emitted, got {statuses:?}"
        );
    }

    #[test]
    fn drive_add_project_followup_narrates_init_and_seed_and_surfaces_the_warning() {
        // Catches a misleading toast and a swallowed warning: the success
        // message must claim the init and seed only when flagged, and a
        // seed_warning must ride alongside as its own warning status.
        let (mut engine, _tmp) = test_engine();
        let reaction = |initialized: bool, seeded: bool, warning: Option<String>| {
            EventReaction::AddProjectAfterInitialCommit {
                path: "/adopted".to_string(),
                name: "Adopted".to_string(),
                branch: "main".to_string(),
                leading_branch: "main".to_string(),
                initialized_repo: initialized,
                seeded_gitignore: seeded,
                seed_warning: warning,
                status_op_id: None,
            }
        };

        let statuses = engine.drive_add_project_followup(&reaction(true, true, None));
        let info = statuses.iter().find(|s| s.tone == "info").expect("info");
        assert!(info.message.contains("Initialized a git repository"));
        assert!(info.message.contains("seeded a starter .gitignore"));

        // Same path is now registered; use fresh paths for the other rungs.
        engine.projects.clear();
        let statuses = engine.drive_add_project_followup(&reaction(true, false, None));
        let info = statuses.iter().find(|s| s.tone == "info").expect("info");
        assert!(info.message.contains("Initialized a git repository"));
        assert!(
            !info.message.contains("seeded"),
            "no seed clause when nothing was seeded, got: {}",
            info.message
        );

        engine.projects.clear();
        let statuses = engine.drive_add_project_followup(&reaction(false, false, None));
        let info = statuses.iter().find(|s| s.tone == "info").expect("info");
        assert!(
            !info.message.contains("Initialized"),
            "the commit-only flow keeps its existing message, got: {}",
            info.message
        );

        engine.projects.clear();
        let statuses =
            engine.drive_add_project_followup(&reaction(true, false, Some("seed failed".into())));
        assert!(
            statuses
                .iter()
                .any(|s| s.tone == "warning" && s.message == "seed failed"),
            "the seed warning must be emitted as its own warning status, got: {statuses:?}"
        );
    }

    #[test]
    fn persist_project_add_is_idempotent_on_path() {
        // The single commit chokepoint must not create a duplicate Project when
        // two adds for the same path race past the request-time validation (e.g.
        // the initial-commit born-race registering while a sibling worker's
        // followup is still in flight).
        let (mut engine, _tmp) = test_engine();
        let add = |id: &str| Command::PersistProject {
            action: Box::new(crate::worker::ProjectPersistenceAction::Add {
                project: sample_project(id, "/same/path"),
                status_message: "added".to_string(),
            }),
            status_op_id: None,
        };
        engine.apply(add("id-a")).expect("first add");
        let second = engine.apply(add("id-b")).expect("second add (same path)");
        assert_eq!(
            engine.projects.len(),
            1,
            "a second add for the same path must not create a duplicate"
        );
        assert_eq!(engine.projects[0].id, "id-a", "the first registration wins");
        // The dedup surfaces an honest message (already registered), not a
        // fabricated "added" narrative.
        let message = added_status_message(&second).expect("Added outcome");
        assert!(
            message.contains("already in the workspace"),
            "dedup must report the truth, got: {message}"
        );
    }

    #[test]
    fn finish_web_project_add_surfaces_the_dedup_message_not_the_callers_narrative() {
        // The full web followup path: when a race means the path is already
        // registered, the status must carry the engine's honest "already in the
        // workspace" text, not the losing caller's fabricated success narrative.
        let (mut engine, _tmp) = test_engine();
        engine
            .apply(Command::PersistProject {
                action: Box::new(crate::worker::ProjectPersistenceAction::Add {
                    project: sample_project("winner", "/p"),
                    status_message: "added".to_string(),
                }),
                status_op_id: None,
            })
            .expect("pre-register the winner");

        let statuses = engine.finish_web_project_add(WebProjectAdd {
            path: "/p",
            display_name: "Loser".to_string(),
            current_branch: "main",
            leading_branch: "main",
            status_message:
                "Created an initial commit and added project \"Loser\" to the workspace."
                    .to_string(),
            add_failed_prefix: "Created the initial commit but couldn't add the project",
            status_op_id: &None,
        });

        assert_eq!(engine.projects.len(), 1, "no duplicate project");
        let msg = &statuses.last().expect("a status").message;
        assert!(
            msg.contains("already in the workspace"),
            "must surface the honest engine message, got: {msg}"
        );
        assert!(
            !msg.contains("Loser"),
            "must not echo the losing caller's narrative, got: {msg}"
        );
    }

    #[test]
    fn create_initial_commit_born_race_during_reload_defers_without_fabricating_success() {
        // During a config-reload barrier, PersistProject is deferred (reaction is
        // `Nothing`). The born fast-path must NOT fabricate a "Project added"
        // success — it returns no status, matching the plain AddProject handler.
        let repo = init_unborn_repo_dir();
        run_in_repo(
            repo.path(),
            &["commit", "-q", "--allow-empty", "-m", "init"],
        );
        let (mut engine, _tmp) = test_engine();
        engine.reloading = true;

        let outcome = engine
            .apply_wire(WireCommand::AddProjectCreateInitialCommit {
                path: repo.path().to_string_lossy().into_owned(),
                name: "Demo".to_string(),
            })
            .expect("dispatch");
        assert!(
            outcome.status.is_none(),
            "a deferred add must not fabricate a success status"
        );
        assert!(
            engine.projects.is_empty(),
            "deferred — the project isn't registered yet"
        );
    }

    fn run_in_repo(dir: &std::path::Path, args: &[&str]) {
        assert!(
            crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir)
                .status()
                .expect("spawn git")
                .success(),
            "git {args:?} failed"
        );
    }

    #[test]
    fn apply_wire_checkout_default_rejects_an_unborn_repo() {
        let repo = init_unborn_repo_dir();
        let (mut engine, _tmp) = test_engine();
        let res = engine.apply_wire(WireCommand::AddProjectCheckoutDefault {
            path: repo.path().to_string_lossy().into_owned(),
            name: "Demo".to_string(),
        });
        assert!(
            res.is_err(),
            "checkout-default of a commit-less repo must be rejected, not registered"
        );
        assert!(engine.projects.is_empty());
    }

    #[test]
    fn apply_wire_add_project_checkout_default_switches_then_adds() {
        // Known default ("main") differs from HEAD ("feature/x"): the wire spawns
        // the switch worker (busy status), the switch moves HEAD to main, and the
        // follow-up registers the project — mirroring the TUI's "Check Out & Add".
        let (_origin, _clone, work) = clone_repo_on_feature_branch("main");
        let (mut engine, _tmp) = test_engine();

        let outcome = engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: work.to_string_lossy().into_owned(),
                name: "Demo".to_string(),
            })
            .expect("add-checkout");
        let busy = outcome.status.expect("busy status");
        assert_eq!(busy.tone, "busy");
        assert!(
            busy.message.contains("Checking out \"main\" in")
                && busy.message.contains("before adding the project"),
            "unexpected busy message: {}",
            busy.message
        );
        let busy_key = busy.key.clone().expect("add busy must be keyed");

        let statuses = drive_add_project_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "info");
        assert!(
            status
                .message
                .contains("Checked out \"main\" and added project \"Demo\" to the workspace."),
            "unexpected message: {}",
            status.message
        );
        // The success final shares the busy's opaque op id, so it REPLACES the
        // spinner on the same key (the migration's core invariant), and the op is
        // consumed from the registry.
        assert_eq!(
            status.key.as_deref(),
            Some(busy_key.as_str()),
            "the add-project final must reuse the busy's op id"
        );
        assert!(
            !engine.pending_web_add_project_ops.contains_key(&busy_key),
            "the add-project op must be consumed once resolved"
        );
        // HEAD actually moved, and the project landed.
        assert_eq!(current_git_branch(&work), "main");
        assert_eq!(engine.projects.len(), 1);
        assert_eq!(engine.projects[0].name, "Demo");
        assert_eq!(engine.projects[0].current_branch, "main");
    }

    #[test]
    fn drive_add_project_followup_reports_rolled_back_add_as_error() {
        // Regression: the add is INLINE, so a config-write failure returns
        // `Ok(EventReaction::Status(error))` (still a Rust `Ok`) after rolling
        // the project back. The follow-up must surface THAT error, not the
        // optimistic "added project" success. A dead writer fails every save.
        let (_origin, _clone, work) = clone_repo_on_feature_branch("main");
        let (mut engine, _tmp) = test_engine();
        engine.config_writer = crate::config_queue::ConfigWriteQueue::with_dead_writer(
            engine.paths.config_path.clone(),
        );

        // The switch worker still runs (it does not touch config), so the
        // checkout busy status is produced as usual.
        engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: work.to_string_lossy().into_owned(),
                name: "Demo".to_string(),
            })
            .expect("add-checkout");

        let statuses = drive_add_project_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(
            status.tone, "error",
            "a rolled-back add must report failure, not success: {status:?}"
        );
        assert!(
            !status
                .message
                .contains("added project \"Demo\" to the workspace"),
            "the optimistic success message leaked on a failed add: {}",
            status.message
        );
        // Pin the specific rollback path: the message must be the config-write
        // rollback text, so a future break that produced an error via a *different*
        // arm (e.g. the apply-level `Err`) can't pass this test by accident.
        assert!(
            status.message.contains("was rolled back"),
            "expected the config-write rollback message, got: {}",
            status.message
        );
        // The inline rollback undid both the in-memory list and the SQLite row,
        // so nothing persisted.
        assert!(engine.projects.is_empty());
        assert!(
            engine
                .session_store
                .load_projects()
                .expect("load projects")
                .is_empty()
        );
    }

    #[test]
    fn apply_wire_add_project_checkout_default_rejects_heuristic() {
        // `git init` repo (no origin/HEAD) on a non-main branch yields the
        // Heuristic warning, which never offers "Check Out & Add". The wire
        // refuses synchronously rather than guessing a default branch.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: repo.path().to_string_lossy().into_owned(),
                name: String::new(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot determine a default branch to check out"),
            "err: {err}"
        );
        // HEAD untouched; no project added.
        assert_eq!(current_git_branch(repo.path()), "feature");
        assert!(engine.projects.is_empty());
    }

    #[test]
    fn apply_wire_add_project_checkout_default_rejects_non_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: dir.path().to_string_lossy().into_owned(),
                name: String::new(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("not a git repository"),
            "err: {err}"
        );
    }

    #[test]
    fn add_project_checkout_default_busy_is_keyed() {
        // Known default ("main") differs from HEAD ("feature/x"): the wire
        // returns a busy carrying the add-project op's opaque correlation id so
        // the web can replace it when the switch worker (or the inline add)
        // resolves the op.
        let (_origin, _clone, work) = clone_repo_on_feature_branch("main");
        let path_str = work.to_string_lossy().to_string();
        let (mut engine, _tmp) = test_engine();

        let outcome = engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: path_str.clone(),
                name: "Demo".to_string(),
            })
            .expect("add-checkout");
        let status = outcome.status.expect("busy status");
        assert_eq!(status.tone, "busy");
        let key = status.key.as_deref().expect("add busy must be keyed");
        assert!(
            key.starts_with("op-"),
            "add-project-checkout busy must carry the StatusOp's opaque id, got {key:?}"
        );
        assert!(
            engine.pending_web_add_project_ops.contains_key(key),
            "the add-project op must be registered under its busy key"
        );
    }

    // Helper: detach HEAD on a path (requires at least one commit).
    fn detach_head(repo: &std::path::Path) {
        let ok = crate::git::test_support::git_command()
            .args([
                "-C",
                repo.to_string_lossy().as_ref(),
                "checkout",
                "--detach",
                "HEAD",
            ])
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git checkout --detach HEAD failed");
    }

    #[test]
    fn add_project_checkout_default_on_detached_with_origin_head_uses_remote_default() {
        // A cloned repo has origin/HEAD set. Detach HEAD, then verify the command
        // succeeds by falling back to the remote default branch.
        let (_origin, _clone, work) = clone_repo_on_feature_branch("main");
        detach_head(&work);
        let (mut engine, _tmp) = test_engine();

        let outcome = engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: work.to_string_lossy().into_owned(),
                name: "Detached".to_string(),
            })
            .expect("should succeed: origin/HEAD resolves to main");
        let status = outcome.status.expect("busy status");
        assert_eq!(status.tone, "busy");
        assert!(
            status.message.contains("main"),
            "busy message should reference the remote default branch 'main', got: {}",
            status.message
        );
        // Drive the chain to confirm success.
        let statuses = drive_add_project_chain(&mut engine);
        let final_status = statuses.last().expect("final status");
        assert_eq!(
            final_status.tone, "info",
            "expected success, got: {:?}",
            final_status
        );
    }

    #[test]
    fn add_project_checkout_default_on_detached_without_origin_head_returns_clear_error() {
        // A local-only repo (no remote) on detached HEAD: no origin/HEAD to fall back
        // to, so the command must bail with a message mentioning "detached".
        let repo = init_repo_on_feature_branch("main");
        detach_head(repo.path());
        let (mut engine, _tmp) = test_engine();

        let err = engine
            .apply_wire(WireCommand::AddProjectCheckoutDefault {
                path: repo.path().to_string_lossy().into_owned(),
                name: String::new(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("detached"),
            "expected 'detached' in error, got: {err}"
        );
        assert!(
            engine.projects.is_empty(),
            "no project should be added on error"
        );
    }

    #[test]
    fn wire_to_command_reload_config_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::ReloadConfig {})
            .expect("reconstruct");
        assert!(matches!(cmd, Command::ReloadConfig));
    }

    #[test]
    fn wire_to_command_recover_config_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::RecoverConfig {})
            .expect("reconstruct");
        assert!(matches!(cmd, Command::RecoverConfig));
    }

    #[test]
    fn wire_to_command_update_project_startup_command_builds_persist_action() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let cmd = engine
            .wire_to_command(WireCommand::UpdateProjectStartupCommand {
                project_id: "p1".to_string(),
                startup_command: Some("echo hi".to_string()),
            })
            .expect("reconstruct");
        match cmd {
            Command::PersistProject { action, .. } => match *action {
                ProjectPersistenceAction::UpdateStartupCommand {
                    project_id,
                    project_name,
                    startup_command,
                } => {
                    assert_eq!(project_id, "p1");
                    assert_eq!(project_name, "p1-name");
                    assert_eq!(startup_command.as_deref(), Some("echo hi"));
                }
                other => panic!("expected UpdateStartupCommand, got {other:?}"),
            },
            _ => panic!("expected Command::PersistProject variant"),
        }
    }

    #[test]
    fn wire_to_command_update_project_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::UpdateProjectStartupCommand {
            project_id: "ghost".to_string(),
            startup_command: None,
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn apply_wire_delete_session_inline_removes_session() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // The web delete path must also drop the activity stamp — this is the
        // engine-side cleanup; no TUI App caller exists on this path to do it.
        engine
            .pty_activity
            .insert("s1".to_string(), std::time::Instant::now());

        let outcome = engine
            .apply_wire(WireCommand::DeleteSession {
                session_id: "s1".to_string(),
                delete_worktree: false,
                delete_branch: None,
            })
            .expect("apply_wire");
        let status = outcome.status.expect("status");
        assert!(
            status.message.contains("Deleted agent"),
            "unexpected status: {}",
            status.message
        );
        assert!(!engine.sessions.iter().any(|s| s.id == "s1"));
        assert!(
            !engine.pty_activity.contains_key("s1-slot"),
            "deleting a session over the wire must clear its activity stamp"
        );
    }

    #[test]
    fn wire_statuses_passes_through_status() {
        let r = EventReaction::Status(StatusUpdate::error("boom"));
        let s = wire_statuses_from_reaction(&r);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tone, "error");
        assert_eq!(s[0].message, "boom");
    }

    #[test]
    fn wire_statuses_launch_failure_view_is_empty_followup_emits_the_final() {
        // Reconnect / force-restart launch failures are resolved per-surface in
        // `drive_web_launch_followup` (see its own tests), so the bare View emits
        // nothing through `wire_statuses_from_reaction`.
        let r =
            EventReaction::AgentLaunchFailedView(Box::new(AgentLaunchFailedOutcome::Reconnect {
                session_id: "s1".to_string(),
                agent_label: "feat".to_string(),
                message: "nope".to_string(),
            }));
        assert!(wire_statuses_from_reaction(&r).is_empty());
    }

    #[test]
    fn wire_statuses_resume_fallback_is_silent() {
        let r = EventReaction::AgentLaunchFailedView(Box::new(
            AgentLaunchFailedOutcome::ResumeFallback,
        ));
        assert!(wire_statuses_from_reaction(&r).is_empty());
    }

    #[test]
    fn wire_statuses_create_view_is_empty_engine_emits_the_final() {
        // The create success / startup-error finals are resolved ENGINE-SIDE
        // against the shared create op and ride alongside the View as a sibling
        // `Status` in the same `Multi`. `wire_statuses_from_reaction` on the bare
        // View therefore emits nothing — the sibling `Status` is surfaced by the
        // `EventReaction::Status` arm. (The engine-side resolution is covered in
        // `engine::events` tests, where the private op registry is accessible.)
        let committed = crate::engine::AgentLaunchReadyOutcome {
            session: sample_session("s1", "p1", "feat"),
            tab_id: "s1".to_string(),
            pty_size: (24, 80),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Launched agent \"feat\".".to_string(),
                startup_result_error: None,
            },
        };
        assert!(
            wire_statuses_from_reaction(&EventReaction::AgentLaunchReadyView(Box::new(committed)))
                .is_empty()
        );

        let startup_failed = crate::engine::AgentLaunchReadyOutcome {
            session: sample_session("s1", "p1", "feat"),
            tab_id: "s1".to_string(),
            pty_size: (24, 80),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::CreatePersistFailed {
                error: "db error".to_string(),
            },
        };
        assert!(
            wire_statuses_from_reaction(&EventReaction::AgentLaunchReadyView(Box::new(
                startup_failed
            )))
            .is_empty()
        );
    }

    #[test]
    fn wire_statuses_flattens_multi() {
        let r = EventReaction::Multi(vec![
            EventReaction::Status(StatusUpdate::info("a")),
            EventReaction::Nothing,
            EventReaction::Status(StatusUpdate::busy("b")),
        ]);
        assert_eq!(wire_statuses_from_reaction(&r).len(), 2);
    }

    #[test]
    fn apply_wire_stage_file_stages_in_real_repo() {
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::StageFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("apply_wire");
        // StageFile dispatches to EventReaction::Nothing -> no status.
        assert!(outcome.status.is_none());

        let staged = crate::git::test_support::git_command()
            .args(["diff", "--cached", "--name-only"])
            .current_dir(repo.path())
            .output()
            .expect("git diff");
        let names = String::from_utf8_lossy(&staged.stdout);
        assert!(names.contains("a.txt"), "staged names: {names}");
    }

    #[test]
    fn apply_wire_set_last_focused_tab_is_silent_and_persists() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let tab = crate::model::AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.session_store.insert_agent_tab(&tab).unwrap();
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);

        let outcome = engine
            .apply_wire(WireCommand::SetLastFocusedTab {
                session_id: "s1".to_string(),
                tab_id: Some("tab-1".to_string()),
            })
            .expect("apply_wire");

        // J3: no toast for a focus-memory write, high frequency and user-paced.
        assert!(outcome.status.is_none());
        assert_eq!(
            engine.sessions[0].last_focused_tab.as_deref(),
            Some("tab-1")
        );

        // Survives projection into the spine's SessionView.
        let spine = engine.spine();
        let view = spine
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .expect("session view");
        assert_eq!(view.last_focused_tab.as_deref(), Some("tab-1"));
    }

    #[test]
    fn apply_wire_set_last_focused_tab_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::SetLastFocusedTab {
                session_id: "ghost".to_string(),
                tab_id: None,
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn wire_watch_changed_files_deserializes_with_string_id() {
        let json = r#"{"command":"watch_changed_files","args":{"session_id":"s1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::WatchChangedFiles {
                session_id: Some("s1".to_string())
            }
        );
    }

    #[test]
    fn wire_watch_changed_files_deserializes_with_null_id() {
        let json = r#"{"command":"watch_changed_files","args":{"session_id":null}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::WatchChangedFiles { session_id: None });
    }

    #[test]
    fn wire_watch_changed_files_deserializes_with_absent_id() {
        // The frontend's clear path may omit the field entirely; `serde(default)`
        // accepts the empty-args form too.
        let json = r#"{"command":"watch_changed_files","args":{}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::WatchChangedFiles { session_id: None });
    }

    #[test]
    fn wire_to_command_watch_changed_files_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::WatchChangedFiles {
                session_id: Some("s1".to_string()),
            })
            .expect("reconstruct");
        match cmd {
            Command::WatchChangedFiles { session_id } => {
                assert_eq!(session_id.as_deref(), Some("s1"));
            }
            _ => panic!("expected Command::WatchChangedFiles variant"),
        }
    }

    #[test]
    fn apply_wire_watch_changed_files_populates_engine_state() {
        // `init_repo` leaves `a.txt` untracked (no commit), so it shows up as an
        // unstaged change once the worktree is watched.
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        // Empty before the watch (the regression).
        assert!(engine.unstaged_files.is_empty());

        let outcome = engine
            .apply_wire(WireCommand::WatchChangedFiles {
                session_id: Some("s1".to_string()),
            })
            .expect("apply_wire");
        // No synchronous status — the changed-files event is the feedback.
        assert!(outcome.status.is_none());

        // The watch is armed immediately, but the changed-files compute now runs
        // OFF the engine actor thread: the lists are empty until the one-shot
        // worker's ChangedFilesReady event drains (as the actor loop does).
        assert_eq!(engine.watched_session_id.as_deref(), Some("s1"));
        assert!(engine.unstaged_files.is_empty());

        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("ChangedFilesReady");
        engine.process_worker_event(event);

        assert_eq!(engine.watched_session_id.as_deref(), Some("s1"));
        assert!(
            engine.unstaged_files.iter().any(|f| f.path == "a.txt"),
            "unstaged should contain a.txt: {:?}",
            engine.unstaged_files
        );
    }

    #[test]
    fn apply_wire_watch_changed_files_null_clears() {
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        engine
            .apply_wire(WireCommand::WatchChangedFiles {
                session_id: Some("s1".to_string()),
            })
            .expect("apply_wire");
        // Drain the off-thread refresh so the lists are populated before we clear.
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("ChangedFilesReady");
        engine.process_worker_event(event);
        assert!(!engine.unstaged_files.is_empty());

        // Clearing is synchronous: `set_watched_session(None)` empties the lists
        // on the actor thread (no worker), so the engine state reflects it at once.
        engine
            .apply_wire(WireCommand::WatchChangedFiles { session_id: None })
            .expect("apply_wire");

        assert!(engine.watched_session_id.is_none());
        assert!(engine.unstaged_files.is_empty());
    }

    /// Like `init_repo`, but also stages and commits the file so HEAD exists
    /// and `current_branch` resolves a normal (born) branch.
    fn init_repo_with_commit() -> tempfile::TempDir {
        let dir = init_repo();
        let run = |args: &[&str]| {
            let ok = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn wire_add_project_deserializes() {
        let json = r#"{"command":"add_project","args":{"path":"/repo","name":"My Project"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::AddProject {
                path: "/repo".to_string(),
                name: "My Project".to_string(),
            }
        );
    }

    #[test]
    fn wire_remove_project_deserializes() {
        let json = r#"{"command":"remove_project","args":{"project_id":"p1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::RemoveProject {
                project_id: "p1".to_string(),
            }
        );
    }

    #[test]
    fn wire_to_command_add_project_builds_persist_add() {
        let repo = init_repo_with_commit();
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::AddProject {
                path: repo.path().to_string_lossy().into_owned(),
                name: String::new(),
            })
            .expect("reconstruct");
        // canonicalize so the assertion survives symlinked temp dirs (e.g. /tmp -> /private/tmp).
        let expected_path = repo.path().canonicalize().unwrap();
        let expected_name = expected_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();
        match cmd {
            Command::PersistProject { action, .. } => match *action {
                ProjectPersistenceAction::Add { project, .. } => {
                    assert_eq!(PathBuf::from(&project.path), expected_path);
                    assert_eq!(project.name, expected_name);
                }
                other => panic!("expected Add, got {other:?}"),
            },
            _ => panic!("expected Command::PersistProject variant"),
        }
    }

    #[test]
    fn wire_to_command_add_project_rejects_non_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::AddProject {
            path: dir.path().to_string_lossy().into_owned(),
            name: String::new(),
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(
            err.to_string().contains("not a git repository"),
            "err: {err}"
        );
    }

    #[test]
    fn apply_wire_add_project_surfaces_success_status() {
        // The direct web add path (no branch checkout). The inline Add returns
        // ProjectPersistenceOutcome(Added), which the generic apply_wire tail would
        // drop (no Status reaction) — apply_wire must explicitly surface the
        // "Added project …" info so the web client gets a confirmation toast.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        let outcome = engine
            .apply_wire(WireCommand::AddProject {
                path: repo.path().to_string_lossy().into_owned(),
                name: "Demo".to_string(),
            })
            .expect("add");
        let status = outcome
            .status
            .expect("a successful add must surface a status");
        assert_eq!(status.tone, "info", "unexpected status: {status:?}");
        assert!(
            status.message.contains("Added project \"Demo\""),
            "unexpected success message: {}",
            status.message
        );
        assert_eq!(engine.projects.len(), 1);
    }

    #[test]
    fn apply_wire_add_project_surfaces_rollback_error() {
        // Direct web add, config write fails: the inline Add rolls back and returns
        // an error Status; apply_wire must surface THAT, not the optimistic success.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        engine.config_writer = crate::config_queue::ConfigWriteQueue::with_dead_writer(
            engine.paths.config_path.clone(),
        );
        let outcome = engine
            .apply_wire(WireCommand::AddProject {
                path: repo.path().to_string_lossy().into_owned(),
                name: "Demo".to_string(),
            })
            .expect("add");
        let status = outcome
            .status
            .expect("a rolled-back add must surface a status");
        assert_eq!(status.tone, "error", "unexpected status: {status:?}");
        assert!(
            !status.message.contains("Added project \"Demo\""),
            "the optimistic success message leaked: {}",
            status.message
        );
        assert!(engine.projects.is_empty());
    }

    #[test]
    fn wire_to_command_remove_project() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let cmd = engine
            .wire_to_command(WireCommand::RemoveProject {
                project_id: "p1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::RemoveProject {
                project_id,
                project_name,
            } => {
                assert_eq!(project_id, "p1");
                assert_eq!(project_name, "p1-name");
            }
            _ => panic!("expected Command::RemoveProject variant"),
        }
    }

    #[test]
    fn wire_to_command_remove_project_with_sessions_cascades() {
        // Removing a project with agents no longer errors — it cascades,
        // deleting the agents' records while keeping their worktrees on disk.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let cmd = engine
            .wire_to_command(WireCommand::RemoveProject {
                project_id: "p1".to_string(),
            })
            .expect("removing a project with agents now cascades, not refused");
        assert!(matches!(cmd, Command::RemoveProject { .. }));
    }

    #[test]
    fn wire_to_command_remove_ghost_project_uses_short_id_name() {
        // A "ghost" project id (orphaned sessions, no project record) must not
        // error with "unknown project"; the name falls back to a short id slice.
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s1", "3fc34174-ghost", "feat"));
        let cmd = engine
            .wire_to_command(WireCommand::RemoveProject {
                project_id: "3fc34174-ghost".to_string(),
            })
            .expect("ghost removal must not error");
        match cmd {
            Command::RemoveProject {
                project_id,
                project_name,
            } => {
                assert_eq!(project_id, "3fc34174-ghost");
                assert_eq!(project_name, "3fc34174");
            }
            _ => panic!("expected Command::RemoveProject"),
        }
    }

    #[test]
    fn wire_create_agent_deserializes() {
        let json = r#"{"command":"create_agent","args":{"project_id":"p1","name":"feature-x"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "feature-x".to_string(),
                copy_uncommitted_changes: None,
                use_existing_branch: false,
            }
        );
    }

    #[test]
    fn wire_to_command_create_agent_builds_request() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        // Non-empty name -> Some(custom_name), use_existing_branch == false.
        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "feature-x".to_string(),
                copy_uncommitted_changes: None,
                use_existing_branch: false,
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject {
                    project,
                    custom_name,
                    use_existing_branch,
                    ..
                } => {
                    assert_eq!(project.id, "p1");
                    assert_eq!(custom_name.as_deref(), Some("feature-x"));
                    assert!(!use_existing_branch);
                }
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }

        // Empty name -> custom_name == None.
        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: String::new(),
                copy_uncommitted_changes: None,
                use_existing_branch: false,
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject { custom_name, .. } => {
                    assert_eq!(custom_name, None);
                }
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    /// An old JSON body without the copy field still deserializes, and the
    /// request falls back to the config default (true).
    #[test]
    fn create_agent_without_copy_field_uses_config_default() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        assert!(engine.config.defaults.copy_uncommitted_changes_by_default);

        let json = r#"{"command":"create_agent","args":{"project_id":"p1","name":""}}"#;
        let wire: WireCommand = serde_json::from_str(json).expect("deserialize");
        let cmd = engine.wire_to_command(wire).expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject {
                    copy_uncommitted_changes,
                    ..
                } => assert!(copy_uncommitted_changes),
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    /// An explicit per-agent false overrides the config default of true.
    #[test]
    fn create_agent_copy_flag_false_overrides_default_true() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        assert!(engine.config.defaults.copy_uncommitted_changes_by_default);

        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: String::new(),
                copy_uncommitted_changes: Some(false),
                use_existing_branch: false,
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject {
                    copy_uncommitted_changes,
                    ..
                } => assert!(!copy_uncommitted_changes),
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_create_agent_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::CreateAgent {
            project_id: "ghost".to_string(),
            name: "feature-x".to_string(),
            copy_uncommitted_changes: None,
            use_existing_branch: false,
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn wire_to_command_create_agent_rejects_invalid_names() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        // Raw wire clients can send anything; each of these must be rejected with
        // an actionable message naming the rules.
        for bad in [
            "has space", // spaces aren't valid (the FE converts; raw wire doesn't)
            "-leading",  // can't start with a dash
            "a//b",      // no consecutive slashes
            "trailing/", // can't end with a slash
            "/leading",  // can't start with a slash
            "naïve",     // non-ASCII dropped
            "emoji😀",   // non-ASCII dropped
            "with.dot",  // '.' isn't whitelisted
        ] {
            let result = engine.wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: bad.to_string(),
                copy_uncommitted_changes: None,
                use_existing_branch: false,
            });
            let err = result.map(|_| ()).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("Invalid agent name"),
                "expected rejection for {bad:?}, got: {msg}"
            );
        }
    }

    #[test]
    fn wire_to_command_create_agent_accepts_valid_names() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        for good in ["feature-x", "feat/sub-thing", "a_b-c", "AbC123"] {
            let cmd = engine
                .wire_to_command(WireCommand::CreateAgent {
                    project_id: "p1".to_string(),
                    name: good.to_string(),
                    copy_uncommitted_changes: None,
                    use_existing_branch: false,
                })
                .unwrap_or_else(|e| panic!("expected {good:?} to be accepted, got: {e}"));
            match cmd {
                Command::DispatchCreateAgentRequest { request, .. } => match *request {
                    CreateAgentRequest::NewProject { custom_name, .. } => {
                        assert_eq!(custom_name.as_deref(), Some(good));
                    }
                    other => panic!("expected NewProject, got {other:?}"),
                },
                _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
            }
        }
    }

    #[test]
    fn wire_to_command_create_agent_empty_after_trim_is_none() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        // Whitespace-only names trim to empty -> None (server auto-generates),
        // which is valid and must NOT trip the backstop.
        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "   ".to_string(),
                copy_uncommitted_changes: None,
                use_existing_branch: false,
            })
            .expect("whitespace-only name should reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject { custom_name, .. } => {
                    assert_eq!(custom_name, None);
                }
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_reorder_sessions_deserializes() {
        let json =
            r#"{"command":"reorder_sessions","args":{"project_id":"p1","session_ids":["b","a"]}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".to_string(), "a".to_string()],
            }
        );
    }

    #[test]
    fn wire_reorder_projects_deserializes() {
        let json = r#"{"command":"reorder_projects","args":{"project_ids":["c","a","b"]}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ReorderProjects {
                project_ids: vec!["c".to_string(), "a".to_string(), "b".to_string()],
            }
        );
    }

    #[test]
    fn wire_to_command_reorder_sessions_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".to_string(), "a".to_string()],
            })
            .expect("reconstruct");
        match cmd {
            Command::ReorderSessions {
                project_id,
                session_ids,
            } => {
                assert_eq!(project_id, "p1");
                assert_eq!(session_ids, vec!["b".to_string(), "a".to_string()]);
            }
            _ => panic!("expected Command::ReorderSessions variant"),
        }
    }

    #[test]
    fn wire_to_command_reorder_projects_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::ReorderProjects {
                project_ids: vec!["c".to_string(), "a".to_string()],
            })
            .expect("reconstruct");
        match cmd {
            Command::ReorderProjects { project_ids } => {
                assert_eq!(project_ids, vec!["c".to_string(), "a".to_string()]);
            }
            _ => panic!("expected Command::ReorderProjects variant"),
        }
    }

    #[test]
    fn apply_wire_reorder_sessions_reorders_and_is_silent() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b"] {
            let session = sample_session(id, "p1", id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
        let outcome = engine
            .apply_wire(WireCommand::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".to_string(), "a".to_string()],
            })
            .expect("apply_wire");
        // Silent success: no status surfaced.
        assert!(outcome.status.is_none());
        let ids: Vec<String> = engine.sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn apply_wire_reorder_sessions_invalid_set_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("a", "p1", "a"));
        engine.sessions.push(sample_session("b", "p1", "b"));
        let res = engine.apply_wire(WireCommand::ReorderSessions {
            project_id: "p1".to_string(),
            session_ids: vec!["a".to_string()],
        });
        assert!(res.is_err());
    }

    #[test]
    fn drive_delete_followup_finishes_on_worktree_removed() {
        // The async deletion path: BeginDeleteSession spawned a git-removal
        // worker and did NOT remove the session yet. When the worker reports
        // success, drive_delete_followup must run FinishDeleteSession to
        // completion and report the "removed its worktree" status. This covers
        // the async glue without needing a real git worktree.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let reaction = EventReaction::WorktreeRemoveSucceeded {
            session_id: "s1".to_string(),
            branches: crate::engine::RemovedBranches::Deleted(crate::git::RemoveResult::default()),
            our_busy_message: None,
        };
        let statuses = engine.drive_delete_followup(&reaction);

        assert!(
            !engine.sessions.iter().any(|s| s.id == "s1"),
            "session should be removed after worktree removal"
        );
        assert_eq!(statuses.len(), 1, "expected one status: {statuses:?}");
        assert_eq!(statuses[0].tone, "info");
        assert!(
            statuses[0].message.contains("Deleted agent")
                && statuses[0].message.contains("removed its worktree"),
            "unexpected status: {}",
            statuses[0].message
        );
    }

    #[test]
    fn drive_web_launch_followup_resolves_reconnect_op_in_place() {
        use crate::engine::AgentLaunchReadyOutcome;
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "p1".into();

        // Mint the web launch op as `reconnect_session` would: its opaque id keys
        // the busy and the eventual final.
        let op = crate::engine::status_op("Launching agent \"feat\"...").resolve_in_handler(
            |o: &crate::engine::LaunchOutcome| crate::engine::launch_outcome_final(o),
        );
        let op_id = op.id().to_string();
        engine.pending_web_launch_ops.insert("s1".into(), op);

        let reaction = EventReaction::AgentLaunchReadyView(Box::new(AgentLaunchReadyOutcome {
            tab_id: session.slot_tab_id().to_string(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".into(),
            },
        }));
        let followup = engine.drive_web_launch_followup(&reaction);
        assert_eq!(followup.statuses.len(), 1);
        assert_eq!(followup.statuses[0].tone, "info");
        assert_eq!(followup.statuses[0].message, "Reconnected.");
        // Replaced in place on the op's opaque id, and the op is consumed.
        assert_eq!(followup.statuses[0].key.as_deref(), Some(op_id.as_str()));
        assert!(engine.pending_web_launch_ops.is_empty());
    }

    #[test]
    fn drive_web_launch_followup_clears_busy_on_session_missing() {
        use crate::engine::AgentLaunchReadyOutcome;
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "p1".into();

        let op = crate::engine::status_op("Launching agent \"feat\"...").resolve_in_handler(
            |o: &crate::engine::LaunchOutcome| crate::engine::launch_outcome_final(o),
        );
        let op_id = op.id().to_string();
        engine.pending_web_launch_ops.insert("s1".into(), op);

        // SessionMissing resolves the op to a CLEAR (no replacement message).
        let reaction = EventReaction::AgentLaunchReadyView(Box::new(AgentLaunchReadyOutcome {
            tab_id: session.slot_tab_id().to_string(),
            session,
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::SessionMissing,
        }));
        let followup = engine.drive_web_launch_followup(&reaction);
        assert!(followup.statuses.is_empty());
        assert_eq!(followup.clear_keys, vec![op_id]);
        assert!(engine.pending_web_launch_ops.is_empty());
    }

    #[test]
    fn drive_web_launch_followup_reconnect_failure_resolves_op_error() {
        let (mut engine, _tmp) = test_engine();
        let op = crate::engine::status_op("Launching agent \"feat\"...").resolve_in_handler(
            |o: &crate::engine::LaunchOutcome| crate::engine::launch_outcome_final(o),
        );
        let op_id = op.id().to_string();
        engine.pending_web_launch_ops.insert("s1".into(), op);

        let reaction =
            EventReaction::AgentLaunchFailedView(Box::new(AgentLaunchFailedOutcome::Reconnect {
                session_id: "s1".into(),
                agent_label: "feat".into(),
                message: "nope".into(),
            }));
        let followup = engine.drive_web_launch_followup(&reaction);
        assert_eq!(followup.statuses.len(), 1);
        assert_eq!(followup.statuses[0].tone, "error");
        assert_eq!(
            followup.statuses[0].message,
            "Reconnect failed for agent \"feat\": nope"
        );
        assert_eq!(followup.statuses[0].key.as_deref(), Some(op_id.as_str()));
    }

    #[test]
    fn drive_web_launch_followup_startup_auto_reopen_failure_is_unkeyed_warning() {
        // No web op is stashed for startup-auto-reopen (it never goes through
        // reconnect_session), so the failure is an UNKEYED warning with the
        // byte-identical message.
        let (mut engine, _tmp) = test_engine();
        let reaction = EventReaction::AgentLaunchFailedView(Box::new(
            AgentLaunchFailedOutcome::StartupAutoReopen {
                session_id: "s1".into(),
                agent_label: "feat".into(),
                message: "boom".into(),
            },
        ));
        let followup = engine.drive_web_launch_followup(&reaction);
        assert_eq!(followup.statuses.len(), 1);
        assert_eq!(followup.statuses[0].tone, "warning");
        assert_eq!(
            followup.statuses[0].message,
            "Couldn't auto-reopen agent \"feat\": boom"
        );
        assert!(followup.statuses[0].key.is_none());
    }

    #[test]
    fn drive_delete_followup_async_vanishes_session_then_resolves_on_completion() {
        // Graceful flow: AsyncStarted means the agent PTY is already SIGTERMed and
        // held for a background reap, so the web VANISHES the session immediately
        // and keeps a keyed busy op for the deferred worktree removal. By the time
        // WorktreeRemoveCompleted reports back, the session is gone, so the op
        // resolves via the "gone" final wording.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let begin = EventReaction::BeginDeleteSessionView(Box::new(
            crate::engine::BeginDeleteSessionView {
                session_id: "s1".to_string(),
                outcome: BeginDeleteSessionOutcome::AsyncStarted {
                    busy_message: "Removing worktree for agent \"feat\"\u{2026}".to_string(),
                },
            },
        ));
        let busy = engine.drive_delete_followup(&begin);
        let busy_key = busy[0].key.clone().expect("busy key");
        assert!(
            !engine.sessions.iter().any(|s| s.id == "s1"),
            "the session vanishes from the spine immediately, not after removal"
        );

        let reaction = EventReaction::WorktreeRemoveSucceeded {
            session_id: "s1".to_string(),
            branches: crate::engine::RemovedBranches::Deleted(crate::git::RemoveResult::default()),
            our_busy_message: None,
        };
        let statuses = engine.drive_delete_followup(&reaction);
        assert_eq!(statuses.len(), 1, "expected one final: {statuses:?}");
        assert_eq!(statuses[0].key.as_deref(), Some(busy_key.as_str()));
        assert_eq!(statuses[0].tone, "info");
        assert_eq!(statuses[0].message, "Agent and worktree removed.");
        assert!(
            engine.pending_delete_ops_web.is_empty(),
            "op must be consumed on resolution"
        );
    }

    #[test]
    fn drive_delete_followup_resolves_op_on_failure() {
        // The async failure path resolves the keyed op into a same-key error with
        // the byte-identical "Worktree delete failed: …" wording.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.sessions.push(session);

        let begin = EventReaction::BeginDeleteSessionView(Box::new(
            crate::engine::BeginDeleteSessionView {
                session_id: "s1".to_string(),
                outcome: BeginDeleteSessionOutcome::AsyncStarted {
                    busy_message: "Removing worktree for agent \"feat\"\u{2026}".to_string(),
                },
            },
        ));
        let busy_key = engine.drive_delete_followup(&begin)[0]
            .key
            .clone()
            .expect("busy key");

        let reaction = EventReaction::WorktreeRemoveFailed {
            session_id: "s1".to_string(),
            message: "fatal: not a git repository".to_string(),
        };
        let statuses = engine.drive_delete_followup(&reaction);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].key.as_deref(), Some(busy_key.as_str()));
        assert_eq!(statuses[0].tone, "error");
        assert_eq!(
            statuses[0].message,
            "Worktree delete failed: fatal: not a git repository",
        );
    }

    #[test]
    fn drive_delete_followup_clears_busy_when_session_already_gone() {
        // Edge: another path (e.g. the session's project was removed, taking its
        // sessions with it) dropped the session before the async git-removal
        // worker reported back. The worktree removal still completed, so the
        // keyed busy toast MUST be resolved with a same-key final rather than
        // stranded forever (it would otherwise time out to a spurious Warning).
        // The busy/final now correlate by the op's opaque id (minted in the
        // AsyncStarted branch), not `delete:{id}`.
        let (mut engine, _tmp) = test_engine();
        // Establish the op by driving the AsyncStarted branch.
        let begin = EventReaction::BeginDeleteSessionView(Box::new(
            crate::engine::BeginDeleteSessionView {
                session_id: "s1".to_string(),
                outcome: BeginDeleteSessionOutcome::AsyncStarted {
                    busy_message: "Removing worktree for agent \"feat\"\u{2026}".to_string(),
                },
            },
        ));
        let busy = engine.drive_delete_followup(&begin);
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].tone, "busy");
        let busy_key = busy[0].key.clone().expect("busy must carry a key");

        // No session present in `engine.sessions`; the worker reports success.
        let reaction = EventReaction::WorktreeRemoveSucceeded {
            session_id: "s1".to_string(),
            branches: crate::engine::RemovedBranches::Deleted(crate::git::RemoveResult::default()),
            our_busy_message: None,
        };
        let statuses = engine.drive_delete_followup(&reaction);
        assert_eq!(
            statuses.len(),
            1,
            "session-gone success path must still emit a final: {statuses:?}"
        );
        assert_eq!(
            statuses[0].key.as_deref(),
            Some(busy_key.as_str()),
            "the final must carry the same key as the busy"
        );
        assert_eq!(statuses[0].message, "Agent and worktree removed.");
        assert_ne!(statuses[0].tone, "busy", "must resolve, not re-show a busy");
    }

    // ---- G2: fork ----------------------------------------------------------

    #[test]
    fn wire_fork_session_deserializes() {
        let json = r#"{"command":"fork_session","args":{"session_id":"s1","name":"feature-y"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ForkSession {
                session_id: "s1".to_string(),
                name: "feature-y".to_string(),
            }
        );
    }

    #[test]
    fn wire_to_command_fork_session_builds_request() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        // `sample_session` sets title = Some("s1-title"); source_label must use it.
        engine.sessions.push(sample_session("s1", "p1", "feat"));

        let cmd = engine
            .wire_to_command(WireCommand::ForkSession {
                session_id: "s1".to_string(),
                name: "feature-y".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest {
                request,
                busy_message,
                ..
            } => match *request {
                CreateAgentRequest::ForkSession {
                    project,
                    source_session,
                    source_label,
                    custom_name,
                } => {
                    assert_eq!(project.id, "p1");
                    assert_eq!(source_session.id, "s1");
                    // session_label precedence: title-or-branch.
                    assert_eq!(source_label, "s1-title");
                    assert_eq!(custom_name.as_deref(), Some("feature-y"));
                    // Busy copy mirrors the TUI fork message.
                    assert!(
                        busy_message.contains("Forking agent \"s1-title\" as \"feature-y\""),
                        "unexpected busy message: {busy_message}"
                    );
                }
                other => panic!("expected ForkSession, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_fork_session_label_falls_back_to_branch() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        session.title = None; // no custom title -> source_label is the branch.
        engine.sessions.push(session);

        let cmd = engine
            .wire_to_command(WireCommand::ForkSession {
                session_id: "s1".to_string(),
                name: "feature-y".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::ForkSession { source_label, .. } => {
                    assert_eq!(source_label, "feat");
                }
                other => panic!("expected ForkSession, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_fork_session_unknown_session_errors() {
        let (engine, _tmp) = test_engine();
        let err = engine
            .wire_to_command(WireCommand::ForkSession {
                session_id: "ghost".to_string(),
                name: "feature-y".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn wire_to_command_fork_session_empty_name_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        // Unlike CreateAgent, a fork requires a name (the worker would otherwise
        // reject it). Mirror the TUI prompt's "cannot be empty" rejection.
        for blank in ["", "   "] {
            let err = engine
                .wire_to_command(WireCommand::ForkSession {
                    session_id: "s1".to_string(),
                    name: blank.to_string(),
                })
                .map(|_| ())
                .unwrap_err();
            assert!(
                err.to_string().contains("cannot be empty"),
                "blank {blank:?} err: {err}"
            );
        }
    }

    #[test]
    fn wire_to_command_fork_session_rejects_invalid_name() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        for bad in ["-leading", "a//b", "trailing/", "with.dot"] {
            let err = engine
                .wire_to_command(WireCommand::ForkSession {
                    session_id: "s1".to_string(),
                    name: bad.to_string(),
                })
                .map(|_| ())
                .unwrap_err();
            assert!(
                err.to_string().contains("Invalid agent name"),
                "bad {bad:?} err: {err}"
            );
        }
    }

    // ---- G2: rename --------------------------------------------------------

    #[test]
    fn wire_rename_session_deserializes() {
        let json = r#"{"command":"rename_session","args":{"session_id":"s1","title":"My agent"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "My agent".to_string(),
            }
        );
    }

    #[test]
    fn apply_wire_rename_session_sets_title_and_persists() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.title = None;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "  My-agent  ".to_string(),
            })
            .expect("apply_wire");
        // Trimmed and stored in memory.
        assert_eq!(
            engine.sessions[0].title.as_deref(),
            Some("My-agent"),
            "title should be trimmed and set"
        );
        let status = outcome.status.expect("rename surfaces a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("My-agent"),
            "msg: {}",
            status.message
        );

        // Persisted: a fresh load from the same SQLite file sees the new title.
        let reloaded = engine
            .session_store
            .load_sessions()
            .expect("reload sessions");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("session row");
        assert_eq!(s.title.as_deref(), Some("My-agent"));
    }

    #[test]
    fn apply_wire_rename_session_empty_clears_title() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.title = Some("old-name".to_string());
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "   ".to_string(),
            })
            .expect("apply_wire");
        // Cleared back to None -> row shows the branch name again.
        assert_eq!(engine.sessions[0].title, None);
        let status = outcome.status.expect("clear surfaces a status");
        assert!(
            status.message.contains("feat"),
            "clear message names the branch: {}",
            status.message
        );

        let reloaded = engine
            .session_store
            .load_sessions()
            .expect("reload sessions");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("session row");
        assert_eq!(s.title, None, "cleared title should persist");
    }

    #[test]
    fn apply_wire_rename_session_rejects_invalid_title() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "bad name!".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("Invalid agent name"), "err: {err}");
    }

    /// A STANDALONE agent's title is a label, not a refname, so the wire
    /// rename is not held to the refname rules either.
    ///
    /// The asymmetry this pins was a one-way door: creation takes a standalone
    /// title verbatim (folder names legally contain spaces and punctuation a
    /// ref cannot), so a title dux itself minted from a folder called
    /// `My Notes` could not be typed back over the wire.
    #[test]
    fn apply_wire_rename_session_takes_a_standalone_title_verbatim() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_standalone_session("sa1", "/home/someone/My Notes");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "sa1".to_string(),
                title: "  My Notes (2026)  ".to_string(),
            })
            .expect("a standalone rename is not a refname");
        assert_eq!(
            engine.sessions[0].title.as_deref(),
            Some("My Notes (2026)"),
            "trimmed, and otherwise taken verbatim"
        );
        let reloaded = engine.session_store.load_sessions().expect("reload");
        assert_eq!(
            reloaded
                .iter()
                .find(|s| s.id == "sa1")
                .expect("row")
                .title
                .as_deref(),
            Some("My Notes (2026)")
        );
    }

    #[test]
    fn apply_wire_rename_session_unknown_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "ghost".to_string(),
                title: "x".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    // ---- G2: reconnect -----------------------------------------------------

    #[test]
    fn wire_reconnect_session_deserializes() {
        let json = r#"{"command":"reconnect_session","args":{"session_id":"s1","force":true}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: true,
            }
        );
    }

    #[test]
    fn apply_wire_reconnect_session_unknown_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "ghost".to_string(),
                force: false,
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn apply_wire_reconnect_session_missing_worktree_errors() {
        let (mut engine, _tmp) = test_engine();
        // sample_session points worktree_path at a path that does not exist.
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: false,
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("no longer exists"), "err: {err}");
    }

    #[test]
    fn apply_wire_reconnect_session_dispatches_launch() {
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        // Point the worktree at a real, existing directory so the
        // worktree-exists pre-check passes.
        let wt = tmp.path().join("wt-s1");
        std::fs::create_dir_all(&wt).unwrap();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = wt.to_string_lossy().to_string();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: false,
            })
            .expect("apply_wire");
        // The launch was dispatched: the in-flight guard is set and a busy
        // status was surfaced.
        assert!(
            engine.is_in_flight(&crate::engine::InFlightKey::AgentLaunch(TabId::new(
                "s1-slot"
            ))),
            "reconnect should mark the launch in-flight"
        );
        let status = outcome.status.expect("reconnect surfaces a busy status");
        assert_eq!(status.tone, "busy");
        assert!(
            status.message.contains("Launching agent \"s1-title\""),
            "msg: {}",
            status.message
        );
    }

    #[test]
    fn apply_wire_reconnect_session_force_uses_fresh_busy_copy() {
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        let wt = tmp.path().join("wt-s1");
        std::fs::create_dir_all(&wt).unwrap();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = wt.to_string_lossy().to_string();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: true,
            })
            .expect("apply_wire");
        let status = outcome
            .status
            .expect("force reconnect surfaces a busy status");
        assert_eq!(status.tone, "busy");
        // Force reconnect uses the "Starting fresh agent" copy, distinct from
        // the normal reconnect's "Launching agent".
        assert!(
            status.message.contains("Starting fresh agent \"s1-title\""),
            "msg: {}",
            status.message
        );
    }

    #[test]
    fn reconnect_session_busy_is_keyed_with_the_web_launch_op_id() {
        // The busy emitted by a successful reconnect dispatch must carry the
        // opaque id of a web launch op stashed in `pending_web_launch_ops` (keyed
        // by session id), so the web can replace it in place when the launch later
        // completes or fails.
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        let wt = tmp.path().join("wt-s1-keyed");
        std::fs::create_dir_all(&wt).unwrap();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = wt.to_string_lossy().to_string();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: false,
            })
            .expect("apply_wire");
        let status = outcome.status.expect("busy status");
        assert_eq!(status.tone, "busy");
        let busy_key = status.key.expect("reconnect busy carries an opaque op id");
        let op = engine
            .pending_web_launch_ops
            .get("s1")
            .expect("the web launch op must be stashed by session id");
        assert_eq!(
            op.id(),
            busy_key,
            "the busy key must match the stashed op's opaque id"
        );
    }

    // ---- L3: change agent provider -----------------------------------------

    #[test]
    fn wire_change_agent_provider_deserializes() {
        let json =
            r#"{"command":"change_agent_provider","args":{"session_id":"s1","provider":"codex"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "codex".to_string(),
            }
        );
    }

    #[test]
    fn apply_wire_change_agent_provider_swaps_and_persists_when_stopped() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat"); // provider "claude"
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "codex".to_string(),
            })
            .expect("apply_wire");
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        let status = outcome.status.expect("swap surfaces a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("will use codex next launch"),
            "msg: {}",
            status.message
        );
        // Not-running + provider never launched here → "fresh session" note.
        assert!(
            status.message.contains("start a fresh session"),
            "msg: {}",
            status.message
        );

        // Persisted: a fresh load from the same SQLite file sees the new provider.
        let reloaded = engine.session_store.load_sessions().expect("reload");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("row");
        assert_eq!(s.provider.as_str(), "codex");
    }

    #[test]
    fn apply_wire_change_agent_provider_same_provider_is_noop() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat"); // provider "claude"
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "claude".to_string(),
            })
            .expect("apply_wire");
        let status = outcome.status.expect("no-op still surfaces a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("already uses claude"),
            "msg: {}",
            status.message
        );
        // Still claude; no spurious updated_at-driven persistence concerns here.
        assert_eq!(engine.sessions[0].provider.as_str(), "claude");
    }

    #[test]
    fn apply_wire_change_agent_provider_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "ghost".to_string(),
                provider: "codex".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn apply_wire_change_agent_provider_rejects_unconfigured_provider() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // "frobnicate" is not in the configured provider list — the server must
        // reject it rather than trusting the client.
        let err = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "frobnicate".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("not configured"), "err: {err}");
        // The session's provider is unchanged after a rejected swap.
        assert_eq!(engine.sessions[0].provider.as_str(), "claude");
    }

    #[test]
    fn apply_wire_change_agent_provider_warns_when_running() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat"); // provider "claude"
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // Spawn a real `cat` PTY so the session counts as running.
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

        let outcome = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "codex".to_string(),
            })
            .expect("apply_wire");
        let status = outcome.status.expect("running swap surfaces a status");
        assert_eq!(status.tone, "warning");
        assert!(
            status.message.contains("still running") && status.message.contains("codex"),
            "msg: {}",
            status.message
        );
        // Provider is persisted to the new one; the pin keeps the old one for labels.
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        assert_eq!(
            engine
                .running_provider_pins
                .get(TabIdRef::new("s1-slot"))
                .map(|p| p.as_str()),
            Some("claude")
        );

        // Clean up so the PTY doesn't outlive the test.
        engine.providers.remove(TabIdRef::new("s1-slot"));
    }

    // ---- L2: attach existing managed worktree -----------------------------

    /// Build a real repo (init + one commit) at `<root>/repo`, then add a managed
    /// worktree on a new branch under the engine's managed root
    /// (`<worktrees_root>/<project_name>/<branch>`) so `list_worktrees` +
    /// `classify_project_worktrees` see a genuine adoptable managed entry.
    /// Returns the project (path -> repo) and the canonical managed-worktree path.
    fn engine_with_managed_worktree(
        engine: &crate::engine::Engine,
        branch: &str,
    ) -> (Project, PathBuf) {
        let project = sample_project("p1", "");
        let repo = engine.paths.root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");
        let run = |cwd: &Path, args: &[&str]| {
            let ok = crate::git::test_support::git_command()
                .args(args)
                .current_dir(cwd)
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed in {}", cwd.display());
        };
        run(&repo, &["init", "-q"]);
        run(&repo, &["config", "user.email", "t@example.com"]);
        run(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "hello\n").expect("write");
        run(&repo, &["add", "a.txt"]);
        run(&repo, &["commit", "-q", "-m", "init"]);

        // Managed worktree under <worktrees_root>/<project.name>/<branch>.
        let managed_root = engine.paths.worktrees_root.join(&project.name);
        std::fs::create_dir_all(&managed_root).expect("managed root");
        let worktree_path = managed_root.join(branch);
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                worktree_path.to_string_lossy().as_ref(),
            ],
        );

        let mut project = project;
        project.path = repo.to_string_lossy().to_string();
        let canonical = worktree_path.canonicalize().expect("canonicalize worktree");
        (project, canonical)
    }

    #[test]
    fn wire_create_agent_from_worktree_deserializes() {
        let json = r#"{"command":"create_agent_from_worktree","args":{"project_id":"p1","worktree_path":"/wt/feat","name":"my-agent"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::CreateAgentFromWorktree {
                project_id: "p1".to_string(),
                worktree_path: "/wt/feat".to_string(),
                name: "my-agent".to_string(),
            }
        );
    }

    #[test]
    fn wire_to_command_create_agent_from_worktree_builds_request() {
        let (mut engine, _tmp) = test_engine();
        let (project, worktree_path) = engine_with_managed_worktree(&engine, "orphan");
        engine.projects.push(project);

        let cmd = engine
            .wire_to_command(WireCommand::CreateAgentFromWorktree {
                project_id: "p1".to_string(),
                worktree_path: worktree_path.to_string_lossy().to_string(),
                name: "Display Name".replace(' ', "-"),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest {
                request,
                busy_message,
                term_size,
            } => {
                assert_eq!(term_size, (80, 24));
                assert!(
                    busy_message.contains("Starting agent \"Display-Name\" in existing worktree"),
                    "busy: {busy_message}"
                );
                match *request {
                    CreateAgentRequest::ExistingManagedWorktree {
                        project,
                        worktree_path: req_path,
                        branch_name,
                        custom_name,
                    } => {
                        assert_eq!(project.id, "p1");
                        assert_eq!(req_path, worktree_path);
                        assert_eq!(branch_name, "orphan");
                        assert_eq!(custom_name.as_deref(), Some("Display-Name"));
                    }
                    other => panic!("expected ExistingManagedWorktree, got {other:?}"),
                }
            }
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_create_agent_from_worktree_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let err = engine
            .wire_to_command(WireCommand::CreateAgentFromWorktree {
                project_id: "ghost".to_string(),
                worktree_path: "/wt/feat".to_string(),
                name: "my-agent".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn wire_to_command_create_agent_from_worktree_rejects_foreign_path() {
        // A path that is not an adoptable managed worktree (here: the project's
        // own repo checkout, which classification excludes) must be rejected —
        // the server never trusts the client's path.
        let (mut engine, _tmp) = test_engine();
        let (project, _managed) = engine_with_managed_worktree(&engine, "orphan");
        let repo_path = project.path.clone();
        engine.projects.push(project);

        let err = engine
            .wire_to_command(WireCommand::CreateAgentFromWorktree {
                project_id: "p1".to_string(),
                worktree_path: repo_path,
                name: "my-agent".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("not an adoptable managed worktree"),
            "err: {err}"
        );
    }

    #[test]
    fn wire_to_command_create_agent_from_worktree_rejects_already_attached() {
        // A managed worktree that already has a live agent session is no longer
        // adoptable; the path must be rejected even though it IS managed.
        let (mut engine, _tmp) = test_engine();
        let (project, managed) = engine_with_managed_worktree(&engine, "orphan");
        engine.projects.push(project);
        // Seed a session on the managed worktree so classification marks it
        // not-selectable.
        let mut session = sample_session("s1", "p1", "orphan");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = managed.to_string_lossy().to_string();
        engine.sessions.push(session);

        let err = engine
            .wire_to_command(WireCommand::CreateAgentFromWorktree {
                project_id: "p1".to_string(),
                worktree_path: managed.to_string_lossy().to_string(),
                name: "my-agent".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("not an adoptable managed worktree"),
            "err: {err}"
        );
    }

    #[test]
    fn wire_to_command_create_agent_from_worktree_rejects_empty_and_invalid_names() {
        let (mut engine, _tmp) = test_engine();
        let (project, managed) = engine_with_managed_worktree(&engine, "orphan");
        engine.projects.push(project);
        let path = managed.to_string_lossy().to_string();

        // The display-name prompt rejects an empty name (mirroring the TUI's
        // NameNewAgent confirm: "Agent name cannot be empty.").
        for blank in ["", "   "] {
            let err = engine
                .wire_to_command(WireCommand::CreateAgentFromWorktree {
                    project_id: "p1".to_string(),
                    worktree_path: path.clone(),
                    name: blank.to_string(),
                })
                .map(|_| ())
                .unwrap_err();
            assert!(
                err.to_string().contains("cannot be empty"),
                "blank {blank:?} err: {err}"
            );
        }

        // And rejects names git's agent-name rules refuse, same as the prompt.
        for bad in ["-leading", "a//b", "trailing/", "with.dot"] {
            let err = engine
                .wire_to_command(WireCommand::CreateAgentFromWorktree {
                    project_id: "p1".to_string(),
                    worktree_path: path.clone(),
                    name: bad.to_string(),
                })
                .map(|_| ())
                .unwrap_err();
            assert!(
                err.to_string().contains("Invalid agent name"),
                "bad {bad:?} err: {err}"
            );
        }
    }

    #[test]
    fn apply_wire_create_agent_from_worktree_dispatches_and_guards_repeat() {
        // Happy path: the first apply_wire dispatches the create worker (busy is
        // async, so the synchronous outcome is empty and the create is in flight);
        // a repeat hits the in-flight guard like the create/fork flows.
        let (mut engine, _tmp) = test_engine();
        let (project, managed) = engine_with_managed_worktree(&engine, "orphan");
        engine.projects.push(project);
        // Make the provider runnable so the create worker doesn't fail spawning.
        engine.config.providers.commands.insert(
            "claude".to_string(),
            crate::config::ProviderCommandConfig {
                command: "cat".to_string(),
                args: vec![],
                resume_args: None,
                ..Default::default()
            },
        );
        let path = managed.to_string_lossy().to_string();

        let first = engine
            .apply_wire(WireCommand::CreateAgentFromWorktree {
                project_id: "p1".to_string(),
                worktree_path: path.clone(),
                name: "adopted".to_string(),
            })
            .expect("first attach");
        assert!(
            first.status.is_none(),
            "create busy is async, not a synchronous outcome: {:?}",
            first.status
        );
        assert!(
            engine.is_in_flight(&InFlightKey::CreateAgent),
            "the create worker should be in flight after dispatch"
        );

        let second = engine
            .apply_wire(WireCommand::CreateAgentFromWorktree {
                project_id: "p1".to_string(),
                worktree_path: path,
                name: "adopted-again".to_string(),
            })
            .expect("second attach");
        let status = second.status.expect("repeat surfaces the in-flight guard");
        assert_eq!(status.tone, "error");
        assert!(
            status.message.contains("already being created or forked"),
            "msg: {}",
            status.message
        );
    }

    // --- CreateAgentFromPr (L1) ---------------------------------------------

    /// Flip the engine into the gh-available state so the PR flow's guard passes.
    fn enable_gh(engine: &mut Engine) {
        engine.github_integration_enabled = true;
        engine.gh_status = crate::model::GhStatus::Available;
    }

    #[test]
    fn wire_create_agent_from_pr_deserializes() {
        let json = r##"{"command":"create_agent_from_pr","args":{"project_id":"p1","pr":"#42","name":"fix"}}"##;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "#42".to_string(),
                name: "fix".to_string(),
            }
        );
    }

    #[test]
    fn wire_create_agent_from_pr_rejected_when_gh_unavailable() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        // gh_status defaults to Unknown and github_integration_enabled to false,
        // so the PR flow is unavailable — the dialog hides it, but a raw client
        // must still be rejected (and must NOT error/panic — graceful refusal).
        let err = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "#42".to_string(),
                name: String::new(),
            })
            .expect_err("gh unavailable");
        assert!(
            err.to_string().contains("requires GitHub integration"),
            "msg: {err}"
        );
    }

    #[test]
    fn wire_create_agent_from_pr_rejects_unknown_project() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        let err = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "missing".to_string(),
                pr: "#42".to_string(),
                name: String::new(),
            })
            .expect_err("unknown project");
        assert!(err.to_string().contains("unknown project"), "msg: {err}");
    }

    #[test]
    fn wire_create_agent_from_pr_rejects_empty_pr() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/repo"));
        let err = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "   ".to_string(),
                name: String::new(),
            })
            .expect_err("empty pr");
        assert!(
            err.to_string()
                .contains("Enter a GitHub PR URL or PR number"),
            "msg: {err}"
        );
    }

    #[test]
    fn wire_create_agent_from_pr_rejects_invalid_name() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        engine.projects.push(sample_project("p1", "/repo"));
        let err = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "#42".to_string(),
                name: "/bad//name".to_string(),
            })
            .expect_err("invalid name");
        assert!(err.to_string().contains("Invalid agent name"), "msg: {err}");
    }

    #[test]
    fn wire_create_agent_from_pr_returns_busy_and_spawns_lookup() {
        let (mut engine, _tmp) = test_engine();
        enable_gh(&mut engine);
        // A real repo with a GitHub origin so the lookup worker can parse the
        // remote and reach the parse stage. The worker shells out to `gh`, which
        // may be absent in CI — the test only asserts the SYNCHRONOUS busy status
        // and that the worker channel receives a PullRequestResolved event
        // (success or failure).
        let repo = init_repo_with_commit();
        let run = |args: &[&str]| {
            let _ = crate::git::test_support::git_command()
                .args(args)
                .current_dir(repo.path())
                .status();
        };
        run(&[
            "remote",
            "add",
            "origin",
            "https://github.com/octocat/Hello-World.git",
        ]);
        let mut project = sample_project("p1", &repo.path().to_string_lossy());
        project.path_missing = false;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CreateAgentFromPr {
                project_id: "p1".to_string(),
                pr: "#42".to_string(),
                name: "my-agent".to_string(),
            })
            .expect("dispatch lookup");
        let status = outcome.status.expect("synchronous busy status");
        assert_eq!(status.tone, "busy");
        assert!(
            status.message.contains("Resolving PR for project"),
            "msg: {}",
            status.message
        );

        // The lookup worker posts exactly one PullRequestResolved (Ok if gh is
        // installed and the PR resolves, Err otherwise — either way the channel
        // delivers it). Block briefly for the spawned thread.
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("lookup worker posts a PullRequestResolved event");
        assert!(
            matches!(event, WorkerEvent::PullRequestResolved { .. }),
            "expected PullRequestResolved"
        );
    }

    /// Drain the worker channel and return the message of the first
    /// `CommandWorkerStarted` (busy) event. `spawn_command_worker` posts the
    /// busy status onto the worker channel (not the returned reaction), so the
    /// create dispatch's busy copy surfaces there.
    fn first_command_busy_message(engine: &Engine) -> String {
        first_command_busy_message_opt(engine)
            .expect("expected a CommandWorkerStarted busy event on the worker channel")
    }

    /// The same drain, for a test asserting that NO worker was started.
    fn first_command_busy_message_opt(engine: &Engine) -> Option<String> {
        while let Ok(event) = engine.worker_rx.try_recv() {
            if let WorkerEvent::CommandWorkerStarted(status) = event {
                return Some(status.message);
            }
        }
        None
    }

    /// A resolved PR with a `custom_name` drives the create dispatch directly
    /// (no name prompt), building a `CreateAgentRequest::PullRequest` with
    /// `use_existing_branch: false` and the carried name.
    #[test]
    fn drive_pr_lookup_followup_dispatches_create_with_carried_name() {
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", &repo.path().to_string_lossy());
        project.path_missing = false;
        engine.projects.push(project.clone());

        let reaction = EventReaction::OpenNewAgentPromptForPr {
            pr: Box::new(crate::worker::ResolvedPullRequest {
                project,
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                number: 42,
                title: "Fix bug".to_string(),
                state: "OPEN".to_string(),
                head_ref_name: "feature/pr-42".to_string(),
                custom_name: Some("my-agent".to_string()),
            }),
            // No registered op (id None), so the followup returns no clear key and
            // resolves nothing — the create busy flows through the worker channel.
            status_op_id: None,
        };
        // The followup dispatches the create worker; the busy status is posted on
        // the worker channel (CommandWorkerStarted), so the followup itself
        // returns no synchronous status on the happy path.
        let followup = engine.drive_pr_lookup_followup(&reaction);
        assert!(
            followup.statuses.is_empty(),
            "create dispatch busy flows via the worker channel, not the return: {:?}",
            followup.statuses
        );
        assert!(
            engine.is_in_flight(&InFlightKey::CreateAgent),
            "the create worker should be in flight after the followup dispatch"
        );
        let busy = first_command_busy_message(&engine);
        assert!(
            busy.contains("my-agent")
                && busy.contains("from PR #42")
                && busy.contains("launching a fresh session"),
            "msg: {busy}"
        );
    }

    /// When the resolved PR carries no custom name (the TUI path), the follow-up
    /// seeds the head branch as the name, matching the TUI prompt default.
    ///
    /// The fallback is now VALIDATED, so this test's meaning has narrowed: it
    /// pins that a head branch which IS a valid agent name still seeds the name
    /// exactly as before. `feature/head` was already such a name, so the
    /// assertions are unchanged; the sibling test below covers the branch that
    /// is not. Keeping both is the point, since validating the fallback must not
    /// cost the ordinary case its no-typing path.
    #[test]
    fn drive_pr_lookup_followup_seeds_a_valid_head_branch_when_name_absent() {
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", &repo.path().to_string_lossy());
        project.path_missing = false;
        engine.projects.push(project.clone());

        let reaction = EventReaction::OpenNewAgentPromptForPr {
            pr: Box::new(crate::worker::ResolvedPullRequest {
                project,
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                number: 7,
                title: "Add feature".to_string(),
                state: "OPEN".to_string(),
                head_ref_name: "feature/head".to_string(),
                custom_name: None,
            }),
            status_op_id: None,
        };
        let _ = engine.drive_pr_lookup_followup(&reaction);
        let busy = first_command_busy_message(&engine);
        assert!(
            busy.contains("feature/head"),
            "head branch should seed the name: {busy}"
        );
    }

    /// The web's explicit PR name is already validated where the request is
    /// built, but the FALLBACK was not: with no name sent, the PR's head branch
    /// went straight through as the agent name. A head branch is a remote's
    /// string, not dux's, and a branch may legally hold characters
    /// `is_valid_agent_name` refuses. Refuse it and say what to do instead,
    /// rather than sanitising it into a name the user never chose and cannot
    /// predict.
    #[test]
    fn drive_pr_lookup_followup_refuses_a_head_branch_that_is_not_a_valid_agent_name() {
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", &repo.path().to_string_lossy());
        project.path_missing = false;
        engine.projects.push(project.clone());

        let reaction = EventReaction::OpenNewAgentPromptForPr {
            pr: Box::new(crate::worker::ResolvedPullRequest {
                project,
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                number: 7,
                title: "Add feature".to_string(),
                state: "OPEN".to_string(),
                // Legal on the remote, not a legal agent name here.
                head_ref_name: "feature/add colons: & spaces".to_string(),
                custom_name: None,
            }),
            status_op_id: None,
        };
        let followup = engine.drive_pr_lookup_followup(&reaction);

        let error = followup
            .statuses
            .iter()
            .find(|s| s.tone == "error")
            .unwrap_or_else(|| panic!("expected a refusal: {:?}", followup.statuses));
        assert!(
            error.message.contains("feature/add colons: & spaces"),
            "the refusal must name the branch it refused: {}",
            error.message
        );
        assert!(
            error.message.contains("name"),
            "the refusal must say a name is what to supply: {}",
            error.message
        );
        assert!(
            first_command_busy_message_opt(&engine).is_none(),
            "no create should have been dispatched"
        );
    }

    #[test]
    fn drive_pr_lookup_followup_ignores_unrelated_reactions() {
        let (mut engine, _tmp) = test_engine();
        let followup = engine.drive_pr_lookup_followup(&EventReaction::Nothing);
        assert!(followup.statuses.is_empty() && followup.clear_keys.is_empty());
    }

    // ── RunMacro / UpdateMacros wire mapping ────────────────────────────────

    #[test]
    fn wire_run_macro_deserializes() {
        let json = r#"{"command":"run_macro","args":{"target_id":"sess-1","name":"greet"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::RunMacro {
                target_id: "sess-1".to_string(),
                name: "greet".to_string(),
            }
        );
    }

    #[test]
    fn wire_update_macros_deserializes() {
        let json = r#"{"command":"update_macros","args":{"entries":[{"name":"greet","text":"hi","surface":"both"}]}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateMacros {
                entries: vec![WireMacroEntry {
                    name: "greet".to_string(),
                    text: "hi".to_string(),
                    surface: "both".to_string(),
                }],
            }
        );
    }

    #[test]
    fn wire_set_tailscale_mode_deserializes() {
        let json = r#"{"command":"set_tailscale_mode","args":{"mode":"auto"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::SetTailscaleMode {
                mode: "auto".to_string()
            }
        );
    }

    #[test]
    fn setting_the_tailscale_mode_saves_the_tri_state_and_refuses_anything_else() {
        let (mut engine, _tmp) = test_engine();
        assert_eq!(
            engine.config.server.tailscale_mode(),
            crate::config::TailscaleMode::Auto
        );

        for (raw, expected) in [
            ("no", crate::config::TailscaleMode::No),
            ("YES", crate::config::TailscaleMode::Yes),
            ("  auto ", crate::config::TailscaleMode::Auto),
        ] {
            let parsed = engine
                .apply_wire(WireCommand::SetTailscaleMode {
                    mode: raw.to_string(),
                })
                .expect("a tri-state value is accepted");
            assert!(
                parsed.status.is_none(),
                "the sentence a user sees names what happened to the listener, which \
                 only the serve layer knows"
            );
            assert_eq!(engine.config.server.tailscale_mode(), expected);
        }

        // A value outside the tri-state is REFUSED, not degraded: this arrives
        // from a picker, so it is a bug rather than a typo in a file.
        let refused = engine
            .apply_wire(WireCommand::SetTailscaleMode {
                mode: "maybe".to_string(),
            })
            .expect_err("an unknown mode must be refused");
        let message = format!("{refused}");
        assert!(message.contains("maybe"), "{message}");
        assert!(
            message.contains("auto") && message.contains("yes") && message.contains("no"),
            "the refusal must list the valid values: {message}"
        );
        assert_eq!(
            engine.config.server.tailscale_mode(),
            crate::config::TailscaleMode::Auto,
            "and must leave the saved value alone"
        );
    }

    #[test]
    fn mutates_config_static_flags_only_bootstrap_config_writes() {
        // The eager-save config mutations that have no disk-reload to drive a
        // `config.changed` signal — the web actor fires it for these.
        assert!(WireCommand::UpdateMacros { entries: vec![] }.mutates_config_static());
        assert!(
            WireCommand::PersistGlobalEnv {
                env: std::collections::BTreeMap::new(),
            }
            .mutates_config_static()
        );
        assert!(WireCommand::SetChangesPaneVisible { visible: true }.mutates_config_static());
        // Without this the raw editor's disk-ahead reconcile never runs after a
        // mode change and no client refetches the bootstrap that carries it.
        assert!(
            WireCommand::SetTailscaleMode {
                mode: "no".to_string()
            }
            .mutates_config_static()
        );
        // A SetInstanceIdentity carrying at least one field may mutate config.
        assert!(
            WireCommand::SetInstanceIdentity {
                title: Some("dux".into()),
                favicon: Some("amber".into()),
            }
            .mutates_config_static()
        );
        assert!(
            WireCommand::SetInstanceIdentity {
                title: Some("dux".into()),
                favicon: None,
            }
            .mutates_config_static()
        );
        // An empty-body SetInstanceIdentity is a true no-op: it must NOT signal a
        // config.changed fan-out (no disk write happens either).
        assert!(
            !WireCommand::SetInstanceIdentity {
                title: None,
                favicon: None,
            }
            .mutates_config_static()
        );
        // ReloadConfig re-reads the whole file and already signals through the
        // reload path; it must NOT double-fire here.
        assert!(!WireCommand::ReloadConfig {}.mutates_config_static());
        // A non-config command never signals a bootstrap refetch.
        assert!(!WireCommand::WatchChangedFiles { session_id: None }.mutates_config_static());
    }

    /// A `SetSettings` with every field `None` (a true no-op patch).
    fn empty_settings_patch() -> WireCommand {
        WireCommand::SetSettings(SettingsPatch::default())
    }

    /// A `SetSettings` carrying only `pr_banner_position`.
    fn pr_banner_position_only_patch(position: &str) -> WireCommand {
        WireCommand::SetSettings(SettingsPatch {
            pr_banner_position: Some(position.to_string()),
            ..Default::default()
        })
    }

    /// A `SetSettings` carrying only `default_provider`.
    fn default_provider_only_patch(provider: &str) -> WireCommand {
        WireCommand::SetSettings(SettingsPatch {
            default_provider: Some(provider.to_string()),
            ..Default::default()
        })
    }

    /// Pins the trick that replaced the presence check's field list: because
    /// every field is an `Option`, "all absent" is exactly `Self::default()`.
    #[test]
    fn settings_patch_any_present_is_false_only_for_an_all_absent_patch() {
        assert!(!SettingsPatch::default().any_present());
        assert!(
            SettingsPatch {
                copy_on_select: Some(false),
                ..Default::default()
            }
            .any_present(),
            "a lone false must still read as present, not as absent"
        );
        assert!(
            SettingsPatch {
                status_clear_seconds: Some(0),
                ..Default::default()
            }
            .any_present(),
            "a lone zero must still read as present"
        );
        assert!(
            SettingsPatch {
                default_provider: Some(String::new()),
                ..Default::default()
            }
            .any_present(),
            "a lone empty string must still read as present"
        );
    }

    /// WIRE PIN. `SetSettings` carries its fields in a newtype payload rather
    /// than inline on the variant. Under this enum's adjacent tagging a newtype
    /// variant flattens its inner struct into `args`, so the envelope is
    /// byte-identical to the inline-field form and existing JSON still parses.
    /// This freezes that property: a future change to the variant's shape that
    /// altered the envelope fails here rather than at a client.
    #[test]
    fn set_settings_wire_json_round_trips_through_args() {
        let json = serde_json::json!({
            "command": "set_settings",
            "args": { "copy_on_select": false },
        });
        let parsed: WireCommand = serde_json::from_value(json.clone()).expect("parse");
        assert_eq!(
            parsed,
            WireCommand::SetSettings(SettingsPatch {
                copy_on_select: Some(false),
                ..Default::default()
            })
        );
        // Absent fields must serialize back out as explicit nulls inside `args`,
        // never leak out of `args`, and re-parse to the same command.
        let reserialized = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(reserialized["command"], "set_settings");
        assert_eq!(reserialized["args"]["copy_on_select"], false);
        let reparsed: WireCommand = serde_json::from_value(reserialized).expect("re-parse");
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn set_settings_mutates_config_static_only_when_a_field_is_present() {
        assert!(!empty_settings_patch().mutates_config_static());
        assert!(pr_banner_position_only_patch("top").mutates_config_static());
        assert!(default_provider_only_patch("codex").mutates_config_static());
    }

    #[test]
    fn a_settings_patch_leaves_the_upload_settings_it_never_names_alone() {
        // `ui.upload_directory` has no Preferences row and deliberately never
        // will (a path needs a directory picker, not a text row).
        // `ui.upload_write_gitignore` now has one, and is kept here as the
        // other half of the same check: a patch that names NEITHER must leave
        // both exactly as they were. A `[ui]` field is only safe if the
        // settings PATCH carries the current value of every field it does not
        // name, so this pins the mechanism rather than the convention:
        // `set_settings` clones the whole running config, mutates only the
        // fields the patch names, and persists that clone.
        let (mut engine, _tmp) = test_engine();
        std::fs::write(
            &engine.paths.config_path,
            "[ui]\nupload_directory = \"tmp/drops\"\nupload_write_gitignore = false\n",
        )
        .expect("seed config");
        engine.config = crate::config::load_config(&engine.paths);
        assert_eq!(engine.config.ui.upload_directory, "tmp/drops");
        assert!(!engine.config.ui.upload_write_gitignore);

        // An UNRELATED `[ui]` row, one the dialog really does have.
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                copy_on_select: Some(false),
                ..Default::default()
            }))
            .expect("apply set_settings");
        engine.config_writer.flush();

        assert_eq!(
            engine.config.ui.upload_directory, "tmp/drops",
            "the running config must keep the value it was not asked to change"
        );
        assert!(!engine.config.ui.upload_write_gitignore);

        let disk =
            std::fs::read_to_string(&engine.paths.config_path).expect("read config after flush");
        assert!(
            disk.contains("copy_on_select = false"),
            "the patch itself must have landed, or this test proves nothing:\n{disk}"
        );
        assert!(
            disk.contains("upload_directory = \"tmp/drops\""),
            "a settings PATCH must not reset a [ui] field it never named:\n{disk}"
        );
        assert!(
            disk.contains("upload_write_gitignore = false"),
            "a settings PATCH must not reset a [ui] field it never named:\n{disk}"
        );
        let reloaded = crate::config::load_config(&engine.paths);
        assert_eq!(reloaded.ui.upload_directory, "tmp/drops");
        assert!(!reloaded.ui.upload_write_gitignore);
    }

    #[test]
    fn set_settings_accepts_a_valid_default_provider_patch() {
        let (mut engine, _tmp) = test_engine();
        let outcome = engine
            .apply_wire(default_provider_only_patch("codex"))
            .expect("dispatch ok");
        assert_eq!(outcome.status.expect("status").tone, "info");
        assert_eq!(engine.config.defaults.provider, "codex");
        // The bootstrap projection picks up the new in-memory value immediately
        // (no restart/reload needed for the running server to reflect it).
        assert_eq!(engine.bootstrap().global_default_provider, "codex");
    }

    #[test]
    fn set_settings_rejects_an_unconfigured_default_provider_and_leaves_config_unchanged() {
        let (mut engine, _tmp) = test_engine();
        let before = engine.config.defaults.provider.clone();
        let err = engine
            .apply_wire(default_provider_only_patch("not-a-real-provider"))
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("not-a-real-provider"), "{err}");
        assert!(err.to_string().contains("not configured"), "{err}");
        assert_eq!(engine.config.defaults.provider, before);
    }

    #[test]
    fn normalize_pr_banner_position_accepts_known_values() {
        assert_eq!(normalize_pr_banner_position("top"), Some("top".to_string()));
        assert_eq!(
            normalize_pr_banner_position("  Bottom "),
            Some("bottom".to_string())
        );
    }

    #[test]
    fn normalize_pr_banner_position_rejects_unknown() {
        assert_eq!(normalize_pr_banner_position("left"), None);
        assert_eq!(normalize_pr_banner_position(""), None);
    }

    #[test]
    fn set_settings_accepts_a_valid_ui_patch() {
        let (mut engine, _tmp) = test_engine();
        let outcome = engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                copy_on_select: Some(false),
                always_show_tab_strip: Some(true),
                pr_banner_position: Some("top".to_string()),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(outcome.status.expect("status").tone, "info");
        assert!(!engine.config.ui.copy_on_select);
        assert!(engine.config.ui.always_show_tab_strip);
        assert_eq!(engine.config.ui.pr_banner_position, "top");
    }

    /// Every field `set_settings` accepts must be copied back into the RUNNING
    /// `self.config`, not just written to disk via `candidate`. Asserting disk
    /// content alone hid a real bug: `enable_randomized_pet_name_by_default`
    /// persisted correctly but left the in-memory config stale, so the engine
    /// kept using the old value (and kept projecting it into the bootstrap)
    /// until the next config reload. This asserts the in-memory value instead.
    #[test]
    fn set_settings_copies_pet_name_default_back_into_the_running_config() {
        let (mut engine, _tmp) = test_engine();
        let before = engine.config.defaults.enable_randomized_pet_name_by_default;
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                enable_randomized_pet_name_by_default: Some(!before),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(
            engine.config.defaults.enable_randomized_pet_name_by_default, !before,
            "the running config must reflect the flip, not just the file on disk"
        );
    }

    #[test]
    fn set_settings_clamps_out_of_range_status_clear_seconds() {
        let (mut engine, _tmp) = test_engine();
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                status_clear_seconds: Some(u16::MAX),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(
            engine.config.ui.status_clear_seconds,
            crate::config::MAX_STATUS_CLEAR_SECONDS
        );
    }

    #[test]
    fn set_settings_degrades_an_out_of_range_terminal_font_size_to_the_default() {
        let (mut engine, _tmp) = test_engine();
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                terminal_font_size: Some(200),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(
            engine.config.ui.terminal_font_size,
            crate::config::DEFAULT_TERMINAL_FONT_SIZE
        );
    }

    #[test]
    fn set_settings_sanitizes_a_terminal_font_family_containing_control_characters() {
        let (mut engine, _tmp) = test_engine();
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                terminal_font_family: Some("Fira Code\n; color: red".to_string()),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(
            engine.config.ui.terminal_font_family,
            "Fira Code; color: red"
        );
    }

    #[test]
    fn set_settings_caps_an_overlong_terminal_font_family_to_200_chars() {
        let (mut engine, _tmp) = test_engine();
        let huge = "a".repeat(10_000);
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                terminal_font_family: Some(huge),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(engine.config.ui.terminal_font_family.chars().count(), 200);
    }

    #[test]
    fn set_settings_accepts_zero_for_attention_grace_seconds() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.attention_grace_seconds = 3;
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                attention_grace_seconds: Some(0),
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert_eq!(engine.config.ui.attention_grace_seconds, 0);
    }

    #[test]
    fn set_settings_rejects_unknown_enum_value_and_leaves_config_unchanged() {
        let (mut engine, _tmp) = test_engine();
        let before = engine.config.ui.pr_banner_position.clone();
        let err = engine
            .apply_wire(pr_banner_position_only_patch("sideways"))
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("PR banner position"), "{err}");
        assert_eq!(engine.config.ui.pr_banner_position, before);
    }

    #[test]
    fn set_settings_empty_patch_is_a_noop() {
        let (mut engine, _tmp) = test_engine();
        let before = engine.config.clone();
        let outcome = engine
            .apply_wire(empty_settings_patch())
            .expect("dispatch ok");
        assert_eq!(
            outcome.status.expect("status").message,
            "Nothing to update."
        );
        assert_eq!(engine.config.ui.copy_on_select, before.ui.copy_on_select);
    }

    #[test]
    fn set_settings_ignores_absent_fields() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.pr_banner_position = "top".to_string();
        engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                copy_on_select: Some(false),
                ..Default::default()
            }))
            .expect("dispatch ok");
        // pr_banner_position was absent from the patch, so it must be untouched.
        assert_eq!(engine.config.ui.pr_banner_position, "top");
    }

    /// A settings patch must disturb nothing outside the fields it carries.
    /// Seeds distinctive values across config areas the patch has no business
    /// touching, applies a single-field patch, and asserts everything else
    /// survived byte for byte. This catches a patch field mapped to the wrong
    /// config field, and it pins that `self.config = candidate` only ever
    /// adopts a candidate differing from the running config in patched fields.
    ///
    /// What it deliberately does NOT catch, stated so nobody mistakes it for a
    /// guard it is not: a future `&mut self` call inserted BETWEEN the clone
    /// and the assign in `set_settings`, whose write the assign would silently
    /// discard. A discarded write is indistinguishable from a write that never
    /// happened, so no black-box test here can observe it. Keeping the clone
    /// and the assign adjacent is what prevents that, not this test.
    #[test]
    fn set_settings_leaves_unpatched_fields_untouched() {
        let (mut engine, _tmp) = test_engine();
        engine.config.macros.entries.insert(
            "greet".to_string(),
            crate::config::MacroEntry {
                text: "hello".to_string(),
                surface: crate::config::MacroSurface::Both,
            },
        );
        engine
            .config
            .env
            .insert("DUX_TEST_KEY".to_string(), "value".to_string());
        engine.config.server.title = "My dux".to_string();
        engine
            .config
            .keys
            .bindings
            .insert("quit".to_string(), vec!["ctrl-q".to_string()]);
        engine.config.shutdown_timeout_seconds = 17;
        let before = engine.config.clone();

        engine
            .apply_wire(pr_banner_position_only_patch("top"))
            .expect("dispatch ok");

        assert_eq!(
            engine.config.ui.pr_banner_position, "top",
            "the patch lands"
        );
        assert_eq!(engine.config.macros, before.macros);
        assert_eq!(engine.config.env, before.env);
        assert_eq!(engine.config.projects, before.projects);
        assert_eq!(engine.config.server, before.server);
        assert_eq!(engine.config.keys, before.keys);
        assert_eq!(engine.config.providers, before.providers);
        assert_eq!(engine.config.defaults, before.defaults);
        assert_eq!(
            engine.config.shutdown_timeout_seconds,
            before.shutdown_timeout_seconds
        );
    }

    /// Pins the whole-struct idempotence compare: a patch whose values already
    /// match the running config reports "Settings unchanged." and writes
    /// nothing. Distinct from the empty-patch no-op, which reports
    /// "Nothing to update." because the client sent no fields at all.
    #[test]
    fn set_settings_unchanged_values_do_not_write() {
        let (mut engine, _tmp) = test_engine();
        engine.config.ui.pr_banner_position = "top".to_string();
        assert!(
            !engine.paths.config_path.exists(),
            "no config file written yet"
        );

        let outcome = engine
            .apply_wire(pr_banner_position_only_patch("top"))
            .expect("dispatch ok");

        assert_eq!(
            outcome.status.expect("status").message,
            "Settings unchanged."
        );
        engine.config_writer.flush();
        assert!(
            !engine.paths.config_path.exists(),
            "an unchanged patch must not touch disk"
        );
    }

    /// The quiet flag: an accessory-bar-only patch that asks for quiet gets NO
    /// status at all — the bar visibly moving is the feedback, and the write
    /// still lands. Suppression is per request: only a request carrying the
    /// flag is silent, so no other settings write can lose its toast.
    #[test]
    fn set_settings_quiet_accessory_bar_patch_emits_no_status() {
        let (mut engine, _tmp) = test_engine();
        let outcome = engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                mobile_accessory_bar: Some(false),
                quiet: true,
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert!(
            outcome.status.is_none(),
            "a quiet accessory-bar write must emit no status"
        );
        assert!(
            !engine.config.ui.mobile_accessory_bar,
            "the write still lands"
        );
    }

    /// Quiet is honored ONLY for a patch confined to the accessory-bar
    /// field. Any other field present keeps the normal status, so the flag
    /// cannot be used to silence an ordinary settings write.
    #[test]
    fn set_settings_quiet_is_ignored_when_the_patch_touches_other_fields() {
        let (mut engine, _tmp) = test_engine();
        let outcome = engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                mobile_accessory_bar: Some(false),
                copy_on_select: Some(false),
                quiet: true,
                ..Default::default()
            }))
            .expect("dispatch ok");
        let status = outcome.status.expect("status kept for a mixed patch");
        assert!(
            status.message.starts_with("Settings updated."),
            "{status:?}"
        );
    }

    /// A quiet accessory-bar patch whose values already match is silent too: the
    /// "Settings unchanged." info is exactly the redundant confirmation the
    /// flag exists to drop.
    #[test]
    fn set_settings_quiet_suppresses_the_unchanged_status_too() {
        let (mut engine, _tmp) = test_engine();
        let already = engine.config.ui.mobile_accessory_bar;
        let outcome = engine
            .apply_wire(WireCommand::SetSettings(SettingsPatch {
                mobile_accessory_bar: Some(already),
                quiet: true,
                ..Default::default()
            }))
            .expect("dispatch ok");
        assert!(outcome.status.is_none());
    }

    /// A patch JSON that predates the quiet flag still deserializes (quiet
    /// defaults to false), so older clients keep their status unchanged.
    #[test]
    fn settings_patch_without_quiet_field_deserializes_as_not_quiet() {
        let patch: SettingsPatch =
            serde_json::from_str(r#"{"mobile_accessory_bar": false}"#).expect("deserialize");
        assert!(!patch.quiet);
        assert_eq!(patch.mobile_accessory_bar, Some(false));
    }

    /// One accepted `SetSettings` field, described end to end: how to seed the
    /// running config with a value that DIFFERS from what the patch sends (so
    /// the row can never pass vacuously), the JSON the patch carries, and how
    /// to read back the config field the value must land in.
    struct SettingsFieldRow {
        /// The key as it appears inside the wire envelope's `args` object.
        key: &'static str,
        seed: fn(&mut crate::config::Config),
        sent: serde_json::Value,
        read: fn(&crate::config::Config) -> String,
        expect: &'static str,
    }

    /// Every field `set_settings` accepts, one row each. Built by hand on
    /// purpose: this table is the loud counterpart to the compiler-enforced
    /// destructure in `set_settings`, and the length assertion below is what
    /// tells a contributor adding a field that a row is owed here too.
    fn settings_field_rows() -> Vec<SettingsFieldRow> {
        vec![
            SettingsFieldRow {
                key: "copy_on_select",
                seed: |c| c.ui.copy_on_select = true,
                sent: serde_json::json!(false),
                read: |c| c.ui.copy_on_select.to_string(),
                expect: "false",
            },
            SettingsFieldRow {
                key: "compose_bar",
                seed: |c| c.ui.compose_bar = "auto".to_string(),
                sent: serde_json::json!("never"),
                read: |c| c.ui.compose_bar.clone(),
                expect: "never",
            },
            SettingsFieldRow {
                key: "mobile_accessory_bar",
                seed: |c| c.ui.mobile_accessory_bar = true,
                sent: serde_json::json!(false),
                read: |c| c.ui.mobile_accessory_bar.to_string(),
                expect: "false",
            },
            SettingsFieldRow {
                key: "upload_write_gitignore",
                seed: |c| c.ui.upload_write_gitignore = true,
                sent: serde_json::json!(false),
                read: |c| c.ui.upload_write_gitignore.to_string(),
                expect: "false",
            },
            SettingsFieldRow {
                key: "upload_pasted_text_chars",
                seed: |c| c.ui.upload_pasted_text_chars = 1_000,
                sent: serde_json::json!(2_500),
                read: |c| c.ui.upload_pasted_text_chars.to_string(),
                expect: "2500",
            },
            SettingsFieldRow {
                key: "auto_reopen_agents",
                seed: |c| c.ui.auto_reopen_agents = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.auto_reopen_agents.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "show_changes_pane",
                seed: |c| c.ui.show_changes_pane = true,
                sent: serde_json::json!(false),
                read: |c| c.ui.show_changes_pane.to_string(),
                expect: "false",
            },
            SettingsFieldRow {
                key: "web_notifications",
                seed: |c| c.capabilities.web_notifications = true,
                sent: serde_json::json!(false),
                read: |c| c.capabilities.web_notifications.to_string(),
                expect: "false",
            },
            SettingsFieldRow {
                key: "always_show_tab_strip",
                seed: |c| c.ui.always_show_tab_strip = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.always_show_tab_strip.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "tab_reaches_agent",
                seed: |c| c.ui.tab_reaches_agent = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.tab_reaches_agent.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "status_clear_seconds",
                seed: |c| c.ui.status_clear_seconds = 6,
                sent: serde_json::json!(11),
                read: |c| c.ui.status_clear_seconds.to_string(),
                expect: "11",
            },
            SettingsFieldRow {
                key: "attention_grace_seconds",
                seed: |c| c.ui.attention_grace_seconds = 3,
                sent: serde_json::json!(9),
                read: |c| c.ui.attention_grace_seconds.to_string(),
                expect: "9",
            },
            SettingsFieldRow {
                key: "attention_indicator",
                seed: |c| c.ui.attention_indicator = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.attention_indicator.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "attention_on_bell",
                seed: |c| c.ui.attention_on_bell = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.attention_on_bell.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "disable_automated_welcome_screen",
                seed: |c| c.ui.disable_automated_welcome_screen = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.disable_automated_welcome_screen.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "disable_release_notes",
                seed: |c| c.ui.disable_release_notes = false,
                sent: serde_json::json!(true),
                read: |c| c.ui.disable_release_notes.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "pr_banner_position",
                seed: |c| c.ui.pr_banner_position = "bottom".to_string(),
                sent: serde_json::json!("top"),
                read: |c| c.ui.pr_banner_position.clone(),
                expect: "top",
            },
            SettingsFieldRow {
                key: "hyperlinks",
                seed: |c| c.capabilities.hyperlinks = false,
                sent: serde_json::json!(true),
                read: |c| c.capabilities.hyperlinks.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "enable_randomized_pet_name_by_default",
                seed: |c| c.defaults.enable_randomized_pet_name_by_default = false,
                sent: serde_json::json!(true),
                read: |c| c.defaults.enable_randomized_pet_name_by_default.to_string(),
                expect: "true",
            },
            SettingsFieldRow {
                key: "default_provider",
                // `codex` ships in `ProvidersConfig::default()`, so it passes the
                // configured-provider validation while differing from the
                // `claude` default.
                seed: |c| c.defaults.provider = "claude".to_string(),
                sent: serde_json::json!("codex"),
                read: |c| c.defaults.provider.clone(),
                expect: "codex",
            },
            SettingsFieldRow {
                key: "terminal_font_family",
                seed: |c| c.ui.terminal_font_family = "Fira Code".to_string(),
                sent: serde_json::json!("Cascadia Code"),
                read: |c| c.ui.terminal_font_family.clone(),
                expect: "Cascadia Code",
            },
            SettingsFieldRow {
                key: "terminal_font_size",
                seed: |c| c.ui.terminal_font_size = 14,
                sent: serde_json::json!(18),
                read: |c| c.ui.terminal_font_size.to_string(),
                expect: "18",
            },
        ]
    }

    /// ENFORCEMENT TEST. Each field must land in the RUNNING config when it is
    /// the ONLY field in the patch. This is the shape that catches a field
    /// missing from the presence check: with a lone field absent from that
    /// check the patch reads as empty and silently no-ops, while an all-fields
    /// patch would still be non-empty and paper over it. That bug shipped once.
    /// The patch is built from the wire JSON rather than a Rust literal so the
    /// row exercises the same envelope a client sends.
    #[test]
    fn set_settings_applies_every_field_when_sent_alone() {
        let rows = settings_field_rows();
        assert_eq!(
            rows.len(),
            22,
            "add a row when you add a field to SettingsPatch"
        );
        for row in rows {
            let (mut engine, _tmp) = test_engine();
            (row.seed)(&mut engine.config);
            assert_ne!(
                (row.read)(&engine.config),
                row.expect,
                "{}: the seed must differ from the value being sent, or the row proves nothing",
                row.key
            );
            let command: WireCommand = serde_json::from_value(serde_json::json!({
                "command": "set_settings",
                "args": { row.key: row.sent },
            }))
            .unwrap_or_else(|err| panic!("{}: build wire command: {err}", row.key));
            engine
                .apply_wire(command)
                .unwrap_or_else(|err| panic!("{}: dispatch: {err}", row.key));
            assert_eq!(
                (row.read)(&engine.config),
                row.expect,
                "{}: sent alone, it must land in the running config",
                row.key
            );
        }
    }

    /// ENFORCEMENT TEST. Every field, in one patch, must land in the RUNNING
    /// config AND on disk, and the two must agree. The existing suite asserted
    /// disk content only, which is how a field missing from the copy-back
    /// shipped: the file updated while the engine kept serving the stale value
    /// until the next reload. The patch is an exhaustive literal with no `..`,
    /// so adding a field fails to COMPILE here rather than silently skipping it.
    #[test]
    fn set_settings_round_trips_every_field_into_the_running_config() {
        let (mut engine, _tmp) = test_engine();
        let before = engine.config.clone();
        // Every value is derived from the current config, so no field can pass
        // by accidentally already holding the value we send.
        let patch = WireCommand::SetSettings(SettingsPatch {
            copy_on_select: Some(!before.ui.copy_on_select),
            compose_bar: Some(if before.ui.compose_bar == "never" {
                "always".to_string()
            } else {
                "never".to_string()
            }),
            mobile_accessory_bar: Some(!before.ui.mobile_accessory_bar),
            upload_write_gitignore: Some(!before.ui.upload_write_gitignore),
            upload_pasted_text_chars: Some(before.ui.upload_pasted_text_chars + 1),
            auto_reopen_agents: Some(!before.ui.auto_reopen_agents),
            show_changes_pane: Some(!before.ui.show_changes_pane),
            web_notifications: Some(!before.capabilities.web_notifications),
            always_show_tab_strip: Some(!before.ui.always_show_tab_strip),
            tab_reaches_agent: Some(!before.ui.tab_reaches_agent),
            status_clear_seconds: Some(before.ui.status_clear_seconds + 1),
            attention_grace_seconds: Some(before.ui.attention_grace_seconds + 1),
            attention_indicator: Some(!before.ui.attention_indicator),
            attention_on_bell: Some(!before.ui.attention_on_bell),
            pr_banner_position: Some(if before.ui.pr_banner_position == "top" {
                "bottom".to_string()
            } else {
                "top".to_string()
            }),
            hyperlinks: Some(!before.capabilities.hyperlinks),
            enable_randomized_pet_name_by_default: Some(
                !before.defaults.enable_randomized_pet_name_by_default,
            ),
            // `claude` is the default, `codex` is the other provider that ships
            // in `ProvidersConfig::default()`.
            default_provider: Some("codex".to_string()),
            disable_automated_welcome_screen: Some(!before.ui.disable_automated_welcome_screen),
            disable_release_notes: Some(!before.ui.disable_release_notes),
            terminal_font_family: Some(format!("{}-x", before.ui.terminal_font_family)),
            terminal_font_size: Some(before.ui.terminal_font_size + 1),
            // Presentation flag, maps to no config field; false keeps this
            // full-patch round trip on the normal (status-emitting) path.
            quiet: false,
        });
        engine.apply_wire(patch).expect("dispatch ok");

        let after = &engine.config;
        assert_eq!(after.ui.copy_on_select, !before.ui.copy_on_select);
        assert_eq!(after.ui.compose_bar, "never");
        assert_eq!(
            after.ui.mobile_accessory_bar,
            !before.ui.mobile_accessory_bar
        );
        assert_eq!(
            after.ui.upload_write_gitignore,
            !before.ui.upload_write_gitignore
        );
        assert_eq!(after.ui.auto_reopen_agents, !before.ui.auto_reopen_agents);
        assert_eq!(after.ui.show_changes_pane, !before.ui.show_changes_pane);
        assert_eq!(
            after.capabilities.web_notifications,
            !before.capabilities.web_notifications
        );
        assert_eq!(
            after.ui.always_show_tab_strip,
            !before.ui.always_show_tab_strip
        );
        assert_eq!(after.ui.tab_reaches_agent, !before.ui.tab_reaches_agent);
        assert_eq!(
            after.ui.status_clear_seconds,
            before.ui.status_clear_seconds + 1
        );
        assert_eq!(
            after.ui.attention_grace_seconds,
            before.ui.attention_grace_seconds + 1
        );
        assert_eq!(after.ui.attention_indicator, !before.ui.attention_indicator);
        assert_eq!(after.ui.attention_on_bell, !before.ui.attention_on_bell);
        assert_ne!(after.ui.pr_banner_position, before.ui.pr_banner_position);
        assert_eq!(
            after.capabilities.hyperlinks,
            !before.capabilities.hyperlinks
        );
        assert_eq!(
            after.defaults.enable_randomized_pet_name_by_default,
            !before.defaults.enable_randomized_pet_name_by_default
        );
        assert_eq!(after.defaults.provider, "codex");
        assert_eq!(
            after.ui.disable_automated_welcome_screen,
            !before.ui.disable_automated_welcome_screen
        );
        assert_eq!(
            after.ui.disable_release_notes,
            !before.ui.disable_release_notes
        );
        assert_eq!(
            after.ui.terminal_font_family,
            format!("{}-x", before.ui.terminal_font_family)
        );
        assert_eq!(
            after.ui.terminal_font_size,
            before.ui.terminal_font_size + 1
        );

        // Disk must agree with memory. Asserting only one of the two is what
        // let the copy-back drift out of step with the eager write.
        engine.config_writer.flush();
        let raw = std::fs::read_to_string(&engine.paths.config_path).expect("read config");
        let disk: crate::config::Config = toml::from_str(&raw).expect("parse config");
        assert_eq!(disk.ui.copy_on_select, after.ui.copy_on_select);
        assert_eq!(disk.ui.compose_bar, after.ui.compose_bar);
        assert_eq!(disk.ui.mobile_accessory_bar, after.ui.mobile_accessory_bar);
        assert_eq!(disk.ui.auto_reopen_agents, after.ui.auto_reopen_agents);
        assert_eq!(disk.ui.show_changes_pane, after.ui.show_changes_pane);
        assert_eq!(
            disk.capabilities.web_notifications,
            after.capabilities.web_notifications
        );
        assert_eq!(
            disk.ui.always_show_tab_strip,
            after.ui.always_show_tab_strip
        );
        assert_eq!(disk.ui.status_clear_seconds, after.ui.status_clear_seconds);
        assert_eq!(
            disk.ui.attention_grace_seconds,
            after.ui.attention_grace_seconds
        );
        assert_eq!(disk.ui.attention_indicator, after.ui.attention_indicator);
        assert_eq!(disk.ui.attention_on_bell, after.ui.attention_on_bell);
        assert_eq!(disk.ui.pr_banner_position, after.ui.pr_banner_position);
        assert_eq!(disk.capabilities.hyperlinks, after.capabilities.hyperlinks);
        assert_eq!(
            disk.defaults.enable_randomized_pet_name_by_default,
            after.defaults.enable_randomized_pet_name_by_default
        );
        assert_eq!(disk.defaults.provider, after.defaults.provider);
        assert_eq!(
            disk.ui.disable_automated_welcome_screen,
            after.ui.disable_automated_welcome_screen
        );
        assert_eq!(
            disk.ui.disable_release_notes,
            after.ui.disable_release_notes
        );
        assert_eq!(disk.ui.terminal_font_family, after.ui.terminal_font_family);
        assert_eq!(disk.ui.terminal_font_size, after.ui.terminal_font_size);
    }

    /// CROSS-LANGUAGE PIN: the curated favicon color names live twice — here in
    /// `CURATED_FAVICON_COLORS` and in the TS `FAVICON_COLORS` map that drives the
    /// customize-webapp dialog and the tinted-duck SVG. A recolor/rename dialog that
    /// offered a color the server rejects (or vice versa) would degrade silently, so
    /// this parses the TS map's keys out of `favicon.ts` and asserts the two sets
    /// are identical. Skips (rather than fails) when the web tree isn't present, e.g.
    /// a published crate build outside the workspace. Copies the file-reading and
    /// relative-path approach from `palette::tests::web_pin_matches_the_typescript_pin`.
    #[test]
    fn curated_favicon_colors_match_the_typescript_list() {
        let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dux-web/web/src/lib/favicon.ts");
        let Ok(source) = std::fs::read_to_string(&ts_path) else {
            eprintln!("skipping: {} not present", ts_path.display());
            return;
        };
        // Isolate the `FAVICON_COLORS` object body, then read the `name: "#hex",`
        // key off each line (the identifier before the first colon).
        let body = source
            .split("export const FAVICON_COLORS: Record<string, string> = {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("FAVICON_COLORS object not found in favicon.ts");
        let mut ts_names: Vec<String> = body
            .lines()
            .filter_map(|line| line.trim().split(':').next())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        ts_names.sort();
        let mut rust_names: Vec<String> = CURATED_FAVICON_COLORS
            .iter()
            .map(|s| s.to_string())
            .collect();
        rust_names.sort();
        assert_eq!(
            rust_names, ts_names,
            "the curated favicon colors drifted between Rust CURATED_FAVICON_COLORS \
             (wire.rs) and the TS FAVICON_COLORS map (favicon.ts). Update BOTH lists \
             together so the dialog and the server agree on the accepted colors."
        );
    }

    #[test]
    fn normalize_instance_favicon_accepts_curated_names() {
        assert_eq!(normalize_instance_favicon("amber"), Some("amber".into()));
        // Trimmed and lowercased.
        assert_eq!(
            normalize_instance_favicon("  VIOLET  "),
            Some("violet".into())
        );
        // Every curated name round-trips.
        for name in CURATED_FAVICON_COLORS {
            assert_eq!(normalize_instance_favicon(name), Some((*name).to_string()));
        }
    }

    #[test]
    fn normalize_instance_favicon_empty_resets_to_default() {
        assert_eq!(normalize_instance_favicon(""), Some(String::new()));
        assert_eq!(normalize_instance_favicon("   "), Some(String::new()));
    }

    #[test]
    fn normalize_instance_favicon_rejects_unknown() {
        // Dropped legacy names, hex, and URLs are all invalid now. `yellow` is the
        // default (empty), not a tint, so it is rejected as an explicit value.
        assert_eq!(normalize_instance_favicon("mauve"), None);
        assert_eq!(normalize_instance_favicon("yellow"), None);
        assert_eq!(normalize_instance_favicon("purple"), None);
        assert_eq!(normalize_instance_favicon("#863bff"), None);
        assert_eq!(normalize_instance_favicon("https://x/y.png"), None);
    }

    #[test]
    fn normalize_instance_title_strips_controls_and_collapses_whitespace() {
        assert_eq!(
            normalize_instance_title("  dux\t\n  prod \r\n "),
            "dux prod"
        );
        // A bare control character becomes nothing meaningful → default.
        assert_eq!(normalize_instance_title("\u{0007}\u{0000}"), "dux");
        // Bidi/format characters (Cf) are neutralized too, not just controls (Cc),
        // so a right-to-left override can't spoof the rendered title.
        assert_eq!(
            normalize_instance_title("invoice\u{202E}cod.exe"),
            "invoice cod.exe"
        );
        assert_eq!(normalize_instance_title("a\u{200D}\u{FEFF}b"), "a b");
    }

    #[test]
    fn normalize_instance_title_empty_resets_to_dux() {
        assert_eq!(normalize_instance_title(""), "dux");
        assert_eq!(normalize_instance_title("     "), "dux");
    }

    #[test]
    fn normalize_instance_title_caps_length_by_chars_not_bytes() {
        // Multi-byte glyphs: 300 of them must cap to 200 chars, never panic on a
        // byte boundary inside a codepoint.
        let input: String = "é".repeat(300);
        let out = normalize_instance_title(&input);
        assert_eq!(out.chars().count(), 200);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn sanitize_terminal_font_family_drops_ascii_control_characters() {
        assert_eq!(
            sanitize_terminal_font_family("Fira Code\n; color: red"),
            "Fira Code; color: red"
        );
        assert_eq!(sanitize_terminal_font_family("\u{0007}\u{0000}"), "");
        assert_eq!(sanitize_terminal_font_family("a\tb\rc"), "abc");
    }

    #[test]
    fn sanitize_terminal_font_family_leaves_ordinary_values_untouched() {
        assert_eq!(sanitize_terminal_font_family("Fira Code"), "Fira Code");
        assert_eq!(
            sanitize_terminal_font_family("\"Cascadia Code\", Consolas"),
            "\"Cascadia Code\", Consolas"
        );
        assert_eq!(sanitize_terminal_font_family(""), "");
    }

    #[test]
    fn sanitize_terminal_font_family_caps_length_by_chars_not_bytes() {
        let input: String = "é".repeat(300);
        let out = sanitize_terminal_font_family(&input);
        assert_eq!(out.chars().count(), 200);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn wire_to_command_run_macro_passes_through() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::RunMacro {
                target_id: "sess-1".to_string(),
                name: "greet".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::RunMacro { target_id, name } => {
                assert_eq!(target_id, "sess-1");
                assert_eq!(name, "greet");
            }
            _ => panic!("expected Command::RunMacro"),
        }
    }

    #[test]
    fn wire_to_command_update_macros_builds_ordered_map() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::UpdateMacros {
                entries: vec![
                    WireMacroEntry {
                        name: "zebra".to_string(),
                        text: "z".to_string(),
                        surface: "agent".to_string(),
                    },
                    WireMacroEntry {
                        name: "alpha".to_string(),
                        text: "a".to_string(),
                        surface: "terminal".to_string(),
                    },
                ],
            })
            .expect("reconstruct");
        match cmd {
            Command::UpdateMacros { macros } => {
                let names: Vec<&String> = macros.entries.keys().collect();
                assert_eq!(names, vec!["zebra", "alpha"]);
                assert_eq!(
                    macros.entries["alpha"].surface,
                    crate::config::MacroSurface::Terminal
                );
            }
            _ => panic!("expected Command::UpdateMacros"),
        }
    }

    /// `Command` does not derive `Debug`, so the validation tests inspect the
    /// `Err` arm directly rather than using `expect_err`/`unwrap_err` (which
    /// require the `Ok` type to be `Debug`).
    fn update_macros_err(engine: &Engine, entries: Vec<WireMacroEntry>) -> String {
        match engine.wire_to_command(WireCommand::UpdateMacros { entries }) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error"),
        }
    }

    #[test]
    fn wire_to_command_update_macros_rejects_empty_name() {
        let (engine, _tmp) = test_engine();
        let err = update_macros_err(
            &engine,
            vec![WireMacroEntry {
                name: "  ".to_string(),
                text: "x".to_string(),
                surface: "both".to_string(),
            }],
        );
        assert!(err.contains("Macro name cannot be empty"), "got: {err}");
    }

    #[test]
    fn wire_to_command_update_macros_rejects_duplicate_name() {
        let (engine, _tmp) = test_engine();
        let err = update_macros_err(
            &engine,
            vec![
                WireMacroEntry {
                    name: "dup".to_string(),
                    text: "a".to_string(),
                    surface: "both".to_string(),
                },
                WireMacroEntry {
                    name: "dup".to_string(),
                    text: "b".to_string(),
                    surface: "both".to_string(),
                },
            ],
        );
        assert!(err.contains("already in use"), "got: {err}");
    }

    #[test]
    fn wire_to_command_update_macros_rejects_empty_text() {
        let (engine, _tmp) = test_engine();
        let err = update_macros_err(
            &engine,
            vec![WireMacroEntry {
                name: "blank".to_string(),
                text: String::new(),
                surface: "both".to_string(),
            }],
        );
        assert!(err.contains("has no text"), "got: {err}");
    }

    #[test]
    fn wire_to_command_update_macros_rejects_unknown_surface() {
        let (engine, _tmp) = test_engine();
        let err = update_macros_err(
            &engine,
            vec![WireMacroEntry {
                name: "weird".to_string(),
                text: "x".to_string(),
                surface: "sideways".to_string(),
            }],
        );
        assert!(err.contains("unknown surface"), "got: {err}");
    }

    #[test]
    fn apply_wire_update_macros_adopts_and_round_trips_through_real_config_writer() {
        // Two things proven here: (1) apply_wire(UpdateMacros) validates, adopts
        // the new macros into the engine config immediately, and eager-saves
        // through the config writer (returning a synchronous success status); (2)
        // the persisted [macros] table preserves user comments via the in-place
        // patch the writer performs.
        let (mut engine, _tmp) = test_engine();
        // Seed an existing config file with a user comment so the in-place patch
        // path runs and comment preservation is meaningful.
        std::fs::write(&engine.paths.config_path, "# user comment\n").expect("seed config");

        let outcome = engine
            .apply_wire(WireCommand::UpdateMacros {
                entries: vec![WireMacroEntry {
                    name: "greet".to_string(),
                    text: "hello\nworld".to_string(),
                    surface: "agent".to_string(),
                }],
            })
            .expect("apply_wire");
        // The eager save reports success synchronously.
        let status = outcome.status.expect("save status");
        assert_eq!(status.tone, "info");

        // The engine adopted the new macros immediately.
        assert!(engine.config.macros.entries.contains_key("greet"));

        // The eager save wrote through the queue; flush so it lands deterministically.
        engine.config_writer.flush();
        let written = std::fs::read_to_string(&engine.paths.config_path).expect("read back");
        assert!(written.contains("[macros]"), "macros table: {written}");
        assert!(written.contains("greet"), "macro name: {written}");
        assert!(written.contains("surface"), "surface key: {written}");
        assert!(
            written.contains("# user comment"),
            "user comment should survive the in-place patch: {written}"
        );

        // The persisted file parses back into a config whose macro matches.
        let reloaded: crate::config::Config = toml::from_str(&written).expect("reparse");
        let entry = reloaded.macros.entries.get("greet").expect("greet entry");
        assert_eq!(entry.text, "hello\nworld");
        assert_eq!(entry.surface, crate::config::MacroSurface::Agent);
    }

    #[test]
    fn wire_statuses_surface_config_reload_failure() {
        let msg = "bad key 'foo' in [keys]".to_string();
        let bare = EventReaction::OpenConfigReloadFailedModal(msg.clone());
        let s = wire_statuses_from_reaction(&bare);
        assert_eq!(s.len(), 1, "bare reload failure must produce one status");
        assert_eq!(s[0].tone, "error");
        assert!(s[0].message.contains("Config reload failed"));
        assert!(s[0].message.contains("bad key 'foo'"));

        // The deferred-reload path wraps it in Multi; the Multi arm recurses.
        let wrapped = EventReaction::Multi(vec![EventReaction::OpenConfigReloadFailedModal(msg)]);
        let s = wire_statuses_from_reaction(&wrapped);
        assert_eq!(
            s.len(),
            1,
            "Multi-wrapped reload failure must still be one status"
        );
        assert_eq!(s[0].tone, "error");
    }

    #[test]
    fn wire_statuses_surface_worktree_list_failure() {
        let bare = EventReaction::ProjectWorktreesArrived {
            project_id: "p1".to_string(),
            result: Err("git worktree list failed".to_string()),
            status_op_id: None,
        };
        let s = wire_statuses_from_reaction(&bare);
        assert_eq!(
            s.len(),
            1,
            "a failed worktree listing must produce one status"
        );
        assert_eq!(s[0].tone, "error");
        assert!(s[0].message.contains("Failed to list worktrees"));

        // A SUCCESSFUL listing must stay silent on the status stream (it has its
        // own ProjectWorktrees reply path); only the Err arm surfaces.
        let ok = EventReaction::ProjectWorktreesArrived {
            project_id: "p1".to_string(),
            result: Ok(Vec::new()),
            status_op_id: None,
        };
        assert!(
            wire_statuses_from_reaction(&ok).is_empty(),
            "a successful worktree listing must not emit a status"
        );
    }

    #[test]
    fn wire_status_sticky_defaults_to_false_and_round_trips() {
        // Every constructor starts non-sticky, the builder is the only way in,
        // and a payload from an older peer that has no `sticky` field at all
        // deserializes to false rather than failing.
        assert!(!WireStatus::new("error", "boom").sticky);
        assert!(!WireStatus::keyed("k", "error", "boom").sticky);
        let marked = WireStatus::new("error", "boom").sticky();
        assert!(marked.sticky);
        let json = serde_json::to_string(&marked).unwrap();
        assert!(json.contains("\"sticky\":true"), "got {json}");
        let back: WireStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, marked);
        let legacy: WireStatus =
            serde_json::from_str(r#"{"tone":"error","message":"boom"}"#).unwrap();
        assert!(!legacy.sticky, "an absent field must read as not sticky");
    }

    #[test]
    fn a_sticky_final_reaches_the_wire_status_unchanged() {
        // The chain a marked call site depends on: `Final::sticky()` sets the
        // flag, `into_reaction` copies it onto the `StatusUpdate`, and
        // `WireStatus::from_update` copies it onto the wire. A break anywhere in
        // that chain silently un-sticks every marked site at once.
        use crate::engine::{EventReaction, Final, ResolvedFinal};
        let reaction = ResolvedFinal::new("k", Final::error("Worktree delete failed.").sticky())
            .into_reaction();
        let EventReaction::Status(update) = reaction else {
            panic!("expected a Status reaction");
        };
        assert!(update.sticky, "the update must carry the flag");
        assert!(
            WireStatus::from_update(&update).sticky,
            "the wire status must carry the flag"
        );
        // The control: an unmarked final of the same tone is not sticky.
        let plain = ResolvedFinal::new("k", Final::error("Push failed.")).into_reaction();
        let EventReaction::Status(plain) = plain else {
            panic!("expected a Status reaction");
        };
        assert!(!plain.sticky);
        assert!(!WireStatus::from_update(&plain).sticky);
    }

    #[test]
    fn wire_status_keyed_constructor_sets_key() {
        let s = WireStatus::keyed("pull", "busy", "Pulling\u{2026}");
        assert_eq!(s.key.as_deref(), Some("pull"));
        assert_eq!(s.tone, "busy");
        assert_eq!(s.message, "Pulling\u{2026}");

        let plain = WireStatus::new("info", "Saved.");
        assert_eq!(plain.key, None);
        // Unkeyed status omits the key field from JSON (skip_serializing_if).
        let json = serde_json::to_string(&plain).unwrap();
        assert!(
            !json.contains("\"key\""),
            "unkeyed status must omit key: {json}"
        );

        // with_key builder should set the key on an existing status.
        let built = WireStatus::new("info", "Done.").with_key("op-123");
        assert_eq!(built.key.as_deref(), Some("op-123"));
        // Keyed status must include the key field in JSON.
        let keyed_json = serde_json::to_string(&s).unwrap();
        assert!(
            keyed_json.contains("\"key\""),
            "keyed status must include key: {keyed_json}"
        );
        assert!(
            keyed_json.contains("\"pull\""),
            "key value must appear in JSON: {keyed_json}"
        );
    }

    #[test]
    fn wire_statuses_key_config_reload_and_worktree_failures() {
        let r = EventReaction::OpenConfigReloadFailedModal("x".into());
        assert_eq!(
            wire_statuses_from_reaction(&r)[0].key.as_deref(),
            Some("config-reload")
        );

        let r = EventReaction::ProjectWorktreesArrived {
            project_id: "p1".into(),
            result: Err("boom".into()),
            status_op_id: None,
        };
        assert_eq!(
            wire_statuses_from_reaction(&r)[0].key.as_deref(),
            Some("worktree-list:p1")
        );
    }

    #[test]
    fn wire_statuses_launch_views_emit_nothing_finals_are_op_resolved() {
        // Every launch View reaction now emits NOTHING through
        // `wire_statuses_from_reaction`: create finals are resolved engine-side and
        // ride as a sibling `Status`; reconnect / force-restart / startup-auto-
        // reopen finals are resolved per-surface in `drive_web_launch_followup`.
        let session = sample_session("s1", "p1", "feat");

        for view in [
            AgentLaunchReadyView::CreateCommitted {
                status_message: "Launched.".to_string(),
                startup_result_error: None,
            },
            AgentLaunchReadyView::CreatePersistFailed {
                error: "db error".to_string(),
            },
            AgentLaunchReadyView::Reconnect {
                status_message: "ok".to_string(),
            },
            AgentLaunchReadyView::SessionMissing,
            AgentLaunchReadyView::StartupAutoReopen,
        ] {
            let outcome = crate::engine::AgentLaunchReadyOutcome {
                tab_id: session.slot_tab_id().to_string(),
                session: session.clone(),
                pty_size: (24, 80),
                detached_session_id: None,
                wants_fullscreen: false,
                view,
            };
            assert!(
                wire_statuses_from_reaction(&EventReaction::AgentLaunchReadyView(Box::new(
                    outcome
                )))
                .is_empty()
            );
        }

        for outcome in [
            AgentLaunchFailedOutcome::Reconnect {
                session_id: "s1".to_string(),
                agent_label: "feat".to_string(),
                message: "nope".to_string(),
            },
            AgentLaunchFailedOutcome::ForceReconnect {
                session_id: "s2".to_string(),
                agent_label: "feat".to_string(),
                message: "nope".to_string(),
            },
            AgentLaunchFailedOutcome::StartupAutoReopen {
                session_id: "s3".to_string(),
                agent_label: "feat".to_string(),
                message: "nope".to_string(),
            },
            AgentLaunchFailedOutcome::Create {
                project_id: Some("p1".to_string()),
                message: "boom".to_string(),
            },
            AgentLaunchFailedOutcome::ResumeFallback,
        ] {
            assert!(
                wire_statuses_from_reaction(&EventReaction::AgentLaunchFailedView(Box::new(
                    outcome
                )))
                .is_empty()
            );
        }
    }

    #[test]
    fn pull_project_routes_through_status_op_correlating_busy_and_final() {
        // The domain-ful pull op declares its messages at dispatch via a
        // StatusOp; the busy and the worker-resolved final share the op's own
        // opaque id (no author-written key), so the web toast is replaced in
        // place rather than stranded.
        let (mut engine, _tmp) = test_engine();
        let repo = std::path::PathBuf::from("/tmp/does-not-exist-pull-wt");
        // spawn_command_worker delivers the busy via a CommandWorkerStarted
        // event and returns Nothing, so drain the busy event first.
        let dispatched = engine
            .apply(Command::Pull {
                repo_path: repo,
                target: crate::worker::PullTarget::Project {
                    project_id: "p1".into(),
                    project_name: "Demo".into(),
                    leading_branch: Some("main".into()),
                },
                busy_message: "Refreshing\u{2026}".into(),
                already_running_message: "Already refreshing.".into(),
            })
            .expect("pull dispatch");
        assert!(matches!(dispatched, EventReaction::Nothing));
        let busy_ev = engine.worker_rx.recv().expect("busy event");
        let busy_id = match engine.process_worker_event(busy_ev) {
            EventReaction::Status(s) => {
                assert_eq!(s.tone, StatusTone::Busy);
                s.key.expect("busy carries an opaque id")
            }
            _ => panic!("expected a pending Busy Status"),
        };
        let ev = engine.worker_rx.recv().expect("completion event");
        match engine.process_worker_event(ev) {
            EventReaction::Status(s) => {
                assert_eq!(
                    s.key.as_deref(),
                    Some(busy_id.as_str()),
                    "the final must carry the same opaque id as the busy"
                );
                // The refresh is best-effort: a failure is a WARNING that says
                // the project continues from the local branch state.
                assert_eq!(s.tone, StatusTone::Warning);
                assert!(
                    s.message
                        .starts_with("Could not refresh \"Demo\" from origin:")
                        && s.message
                            .contains("Continuing from the local branch state."),
                    "unexpected message: {}",
                    s.message
                );
            }
            _ => panic!("expected a correlated warning final"),
        }
    }

    #[test]
    fn push_routes_through_status_op_correlating_busy_and_final() {
        // Push dispatches through a StatusOp: the pending Busy and the
        // worker-resolved final share the op's own opaque id so the web toast is
        // replaced in place rather than stranded.
        let (mut engine, _tmp) = test_engine();
        let worktree = std::path::PathBuf::from("/tmp/does-not-exist-push-wt");

        let pending = engine
            .apply(Command::Push {
                worktree_path: worktree,
            })
            .expect("push dispatch");
        let busy_id = match pending {
            EventReaction::Status(s) => {
                assert_eq!(s.tone, StatusTone::Busy);
                s.key.expect("busy carries an opaque id")
            }
            _ => panic!("expected a pending Busy Status"),
        };

        // The worker fails (bogus path); its resolved final must carry the id.
        let ev = engine.worker_rx.recv().expect("completion event");
        match engine.process_worker_event(ev) {
            EventReaction::Status(s) => {
                assert_eq!(
                    s.key.as_deref(),
                    Some(busy_id.as_str()),
                    "the final must carry the same opaque id as the busy"
                );
                assert_eq!(s.tone, StatusTone::Error);
                assert!(
                    s.message.starts_with("Push to remote failed:"),
                    "unexpected message: {}",
                    s.message
                );
            }
            _ => panic!("expected a correlated error final"),
        }
    }

    #[test]
    fn create_agent_failed_event_resolves_the_op_on_its_opaque_id() {
        // CreateAgentFailed resolves the shared create op so the web dismisses the
        // "Creating a new agent…" spinner in place, replaced by the error final on
        // the op's own opaque id.
        let (mut engine, _tmp) = test_engine();
        let op = crate::engine::status_op("Creating a new agent\u{2026}").resolve_in_handler(
            |o: &crate::engine::CreateLaunchOutcome| match o {
                crate::engine::CreateLaunchOutcome::Failed { message } => {
                    crate::engine::Final::error(message.clone())
                }
                _ => crate::engine::Final::clear(),
            },
        );
        let op_id = op.id().to_string();
        engine.pending_create_ops.insert(op_id.clone(), op);

        let reaction = engine.process_worker_event(WorkerEvent::CreateAgentFailed {
            status_op_id: op_id.clone(),
            message: "worker panicked".to_string(),
        });
        // Project the engine reaction through the wire to confirm the web sees the
        // keyed error on the op's id.
        let statuses = wire_statuses_from_reaction(&reaction);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].key.as_deref(), Some(op_id.as_str()));
        assert_eq!(statuses[0].tone, "error");
        assert_eq!(statuses[0].message, "worker panicked");
    }
}
