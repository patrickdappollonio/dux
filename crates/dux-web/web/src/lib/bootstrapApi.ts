// HTTP client for the build-static / config-derived bootstrap document. Like
// `changesApi.ts`, this is a plain GET (the read-only `git.ts` pattern, with
// `credentials: "same-origin"`) so it composes with HTTP caching and reads as a
// resource fetch. The matching `config.changed` event over `/ws/events` tells
// the client WHEN to re-GET.
//
// These fields used to ride the broadcast `ViewModel`; they are static
// per server config, so a volatile-data channel was the wrong home for them.
// The server is authoritative: it projects the config + runtime capabilities
// into this single document. A non-2xx is thrown as a `BootstrapFetchError`
// carrying the HTTP status so the caller can branch.

import type { FlatSortKey } from "./flatList"
import type { MacroView } from "./types"

// The bootstrap document. Field names/types mirror the server's JSON (snake_case)
// and the values the legacy ViewModel carried, so consumers move over without a
// shape change. Newer fields may be absent when talking to an older server (a `?`
// marks the ones typed optional, e.g. `title`); consumers fall back to the
// per-field documented default rather than assuming every field is present.
export interface Bootstrap {
  /** Configured agent providers (the new-agent / change-provider pickers). */
  available_providers: string[]
  /** Each configured provider's `web_dragdrop_paste` form, normalized server-side
   * to one of the names `DragDropPasteForm` knows. This is how the terminal pane
   * learns what shape to give a DROPPED file's path for the provider running in
   * it, since the agent CLIs do not agree on how they read a pasted path. Keyed by
   * provider name; a name absent from the map means "bare", as does the whole
   * field being absent on an older server (see `dragDropPasteFormFor`). This is
   * a plain projection of config; what a live process launched with is in
   * `tab_web_dragdrop_paste`, and the pane prefers that. */
  provider_web_dragdrop_paste?: Record<string, string>
  /** The form each LIVE tab actually launched with, keyed by TAB ID (same
   * normalized names as `provider_web_dragdrop_paste`). The terminal pane
   * prefers this over the provider map, because two live tabs of one provider
   * can need different forms: launch a tab, edit that provider's
   * `web_dragdrop_paste`, launch another, and both processes report the same
   * provider name, so a provider-keyed map has one slot for two answers. It
   * also covers a tab whose `[providers.<name>]` block the user has since
   * renamed or deleted. A tab absent here (one launched since the last
   * bootstrap fetch) falls back to the provider map, then to "bare"; older
   * servers omit the field entirely, which is the same fallback. */
  tab_web_dragdrop_paste?: Record<string, string>
  /** Text macros from `[macros]` in config order (the macro popover/editor). */
  macros: MacroView[]
  /** The rotating welcome tips shown on the empty-state screen. */
  welcome_tips: string[]
  /** The binary's display version ('vX.Y.Z' or 'development'); shown in the sidebar. */
  dux_version: string
  /** Whether the new-agent name dialog pre-checks "Use randomized pet name". */
  randomize_agent_names_by_default: boolean
  /** Whether the new-agent dialog pre-checks "Copy uncommitted changes from
   * the project checkout". Older servers omit it; consumers fall back to true. */
  copy_uncommitted_changes_by_default?: boolean
  /** Whether the new-agent-from-PR flow is available (GitHub integration + `gh`). */
  gh_available: boolean
  /** Raw `config.ui.github_integration` flag (distinct from `gh_available`, the
   * composite). The palette hides the PR-banner-position command when this is
   * false — i.e. when integration is OFF, not merely when `gh` is unreachable. */
  github_integration: boolean
  /** Mirrors `config.ui.copy_on_select`: whether selecting text in the web
   * terminal auto-copies it to the clipboard (default true). */
  copy_on_select: boolean
  /** Mirrors `config.ui.compose_bar`: whether the mobile terminal shows the
   * compose bar (a buffered textarea with native autocorrect whose Send
   * delivers the message plus a submitting Enter) and redirects a tap on the
   * terminal into it. When false, a tap focuses xterm directly, the
   * pre-compose-bar behavior. Older servers omit it, so consumers fall back
   * to true. */
  compose_bar?: boolean
  /** Mirrors `config.ui.auto_reopen_agents`: the GLOBAL startup auto-reopen
   * switch. When on, agents that were still running when dux last exited (and
   * have their per-agent opt-in) relaunch at the next startup, on the TUI and
   * on `dux serve` alike. Older servers omit it, so consumers fall back to
   * FALSE, the config default (unlike `compose_bar`'s true). */
  auto_reopen_agents?: boolean
  /** Mirrors `config.ui.attention_grace_seconds`: seconds the attention
   * indicators stay visible after the browser tab returns to the foreground,
   * before the focused agent's needs-attention flag clears (default 3; 0
   * clears immediately). Older servers omit it, so consumers fall back to 3. */
  attention_grace_seconds?: number
  /** Mirrors `config.capabilities.web_notifications`: whether the web UI bridges
   * an agent's notification sequences to a browser desktop Notification. Still
   * gated on visitor permission and a backgrounded tab. Older servers omit it,
   * so consumers fall back to true. */
  web_notifications?: boolean
  /** Mirrors `config.capabilities.hyperlinks`: whether the web terminal renders
   * OSC 8 hyperlinks as clickable (http/https only). Older servers omit it, so
   * consumers fall back to true. */
  hyperlinks?: boolean
  /** Mirrors `config.capabilities.clipboard_passthrough` (normalized): whether an
   * agent's OSC 52 clipboard SET reaches the visitor's browser clipboard:
   * "focused"/"always" write it (the browser still requires the tab to have
   * focus), "off" never does. Older servers omit it, so consumers fall back to
   * "focused". */
  clipboard_passthrough?: "focused" | "always" | "off"
  /** Mirrors `config.ui.pr_banner_position`: "bottom" places the PR lane below
   * the terminal, anything else above. (Server sends a free string; the two
   * known values are the only ones the UI branches on.) */
  pr_banner_position: "top" | "bottom"
  /** Mirrors `config.ui.agent_sort`: the flat agent-list sort mode, persisted
   * server-side so it survives restarts and every client agrees. Older servers
   * omit it, so consumers fall back to "active". */
  agent_sort?: FlatSortKey
  /** Mirrors `config.ui.agent_scrollback_lines`; sizes each xterm.js instance. */
  agent_scrollback_lines: number
  /** Mirrors `config.ui.show_changes_pane`; the desktop Changes-pane default. */
  show_changes_pane: boolean
  /** Mirrors `config.ui.always_show_tab_strip`: when true the agent tab strip
   * renders even with a single tab (default false, matching today's chrome-free
   * single-tab pane). */
  always_show_tab_strip: boolean
  /** Global environment variables applied to every spawned agent/terminal. */
  global_env: Record<string, string>
  /** Mirrors `config.ui.status_clear_seconds`: how long an info/success toast
   * stays before auto-clearing. It is the BASE for every tone, not just
   * info/success: `lib/statusToast.ts` scales warning and error off it. 0 means
   * "never auto-clear" for final states. Older servers omit it, so consumers
   * fall back to 6. */
  status_clear_seconds: number
  /** The operator-chosen display name for this dux instance (`config.server
   * .title`). Shown as the browser tab title and the projects-pane wordmark.
   * Optional: older servers omit it, so consumers resolve a missing/blank value
   * to "dux" via `resolveInstanceTitle`. */
  title?: string
  /** The operator-chosen favicon for this dux instance (`config.server.favicon`).
   * Empty/missing shows the bundled full-colour duck (`/favicon.png`); a curated
   * tint colour name recolours the duck silhouette in that colour; anything else
   * (a legacy hex or URL, a dropped colour name) degrades to the default duck with
   * a one-time notice. Resolved and applied by `applyFavicon`. Optional: older
   * servers omit it. */
  favicon?: string
  /** Mirrors `config.ui.agent_tabs_max` (normalized): the per-agent tab cap
   * INCLUDING the session-slot tab. The tab strip disables its "+" once a session has
   * this many tabs; the server re-enforces on create. Older servers omit it, so
   * consumers fall back to `DEFAULT_AGENT_TABS_MAX`. */
  agent_tabs_max?: number
  /** Mirrors `config.ui.attention_indicator`: whether an attention cue is
   * shown at all when an agent asks for attention (default true). Read by the
   * Settings modal's "Both surfaces" group. Older servers omit it, so
   * consumers fall back to true. */
  attention_indicator?: boolean
  /** Mirrors `config.ui.attention_on_bell`: whether a plain terminal bell also
   * counts as an attention request (default true; no effect when
   * `attention_indicator` is false). Older servers omit it, so consumers fall
   * back to true. */
  attention_on_bell?: boolean
  /** Mirrors `config.defaults.provider`: the GLOBAL default provider for new
   * agents in projects without a project-specific override, matching the
   * TUI's `change-default-provider` palette command. Distinct from a
   * project's own `default_provider` override (see `ProjectView` in
   * `types.ts`), which is the effective per-project value. Older servers omit
   * it, so consumers fall back to "claude". */
  global_default_provider?: string
  /** The first-run welcome screen's copy, from `dux_core::welcome_screen` so the
   * web and the TUI say identical words. Present unconditionally, not only when
   * the welcome is pending: the app menu can open the screen on demand. Older
   * servers omit it, so consumers must tolerate `undefined` (the menu entry then
   * has nothing to show). Distinct from `welcome_tips`, the rotating idle-pane
   * tips. */
  welcome_screen?: WelcomeScreenView
  /** `dux_core::urls::WEBSITE` — where the welcome screen's secondary button
   * goes. Server-projected so the two surfaces cannot disagree about a dux URL.
   * Older servers omit it. */
  website_url?: string
  /** The first-load screen THIS launch should show, or null/absent for neither.
   * Decided once by the server at startup and held in its memory, so a browser
   * that connects at any point still receives it. Dismissing it (`dismissFirstLoad`)
   * records the version as seen in SQLite, which the TUI reads too — so a
   * dismissal here settles the screen on both surfaces. */
  pending_first_load?: PendingFirstLoad | null
  /** Mirrors `config.ui.disable_automated_welcome_screen`: suppresses the
   * AUTOMATIC first-run welcome only; the app menu entry still opens it. Older
   * servers omit it, so consumers fall back to false. */
  disable_automated_welcome_screen?: boolean
  /** Mirrors `config.ui.disable_release_notes`: suppresses the AUTOMATIC
   * what's-new screen only; the app menu entry still opens it. Older servers omit
   * it, so consumers fall back to false. */
  disable_release_notes?: boolean
  /** Mirrors `config.server.file_drop_max_bytes`: the per-file size cap for a
   * file dropped onto a pane, where 0 switches file drop OFF. The terminal pane
   * gates its whole drag surface on this, so a disabled feature offers nothing
   * rather than advertising a drop target and collecting a server refusal after
   * the fact (the server refusal remains the real enforcement). Older servers
   * omit it, so consumers fall back to treating file drop as ON. */
  file_drop_max_bytes?: number
}

/** One numbered getting-started step. The number is carried by the server, not
 * derived from the array index. */
export interface WelcomeStepView {
  number: number
  title: string
  detail: string
}

/** The first-run welcome screen's content. Plain prose and titles: the server
 * hands over text, never Markdown, so nothing here needs a Markdown renderer. */
export interface WelcomeScreenView {
  tagline: string
  paragraphs: string[]
  steps: WelcomeStepView[]
}

/** One release's notes, trimmed server-side to what the what's-new screen shows.
 * `paragraphs` and `sections` are plain text (core stripped the Markdown). */
export interface ReleaseNotesView {
  version: string
  headline: string
  paragraphs: string[]
  sections: string[]
  /** The release's own web page — where "Open full notes" goes. */
  html_url: string
}

/** The pending first-load screen. `notes` is present exactly when `screen` is
 * `"whats_new"`: the server never offers that screen without notes in hand. */
export interface PendingFirstLoad {
  screen: "welcome" | "whats_new"
  notes?: ReleaseNotesView | null
}

/** Fallback per-agent tab cap when the server omits `agent_tabs_max` (older
 * servers). Mirrors `dux_core::config::DEFAULT_AGENT_TABS_MAX`
 * (`crates/dux-core/src/config.rs`) — this is a plain duplicated literal, not
 * generated from the Rust constant, so nothing enforces the two staying equal.
 * `bootstrapApi.test.ts` pins this value so a change here (or there) without
 * updating the other shows up as a failing test rather than a silent drift;
 * if you bump one, bump the other in the same change. */
export const DEFAULT_AGENT_TABS_MAX = 20

// A failed bootstrap fetch. `status` is the HTTP status (0 for a network/
// transport failure with no response). The boot path swallows this and keeps the
// last-known bootstrap (null on first boot); a later `config.changed` event or a
// reconnect retries.
export class BootstrapFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "BootstrapFetchError"
    this.status = status
  }
}

export async function fetchBootstrap(): Promise<Bootstrap> {
  let resp: Response
  try {
    resp = await fetch("/api/v1/bootstrap", { credentials: "same-origin" })
  } catch {
    // The request never reached the server (offline, DNS, CORS).
    throw new BootstrapFetchError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new BootstrapFetchError(
      detail || `request failed (${resp.status})`,
      resp.status,
    )
  }
  return (await resp.json()) as Bootstrap
}
