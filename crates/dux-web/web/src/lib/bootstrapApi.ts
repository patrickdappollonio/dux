// HTTP client for the config-derived bootstrap document: a plain same-origin GET,
// re-issued when a `config.changed` event arrives. The server projects config and
// runtime capabilities into this one document and is authoritative.
//
// A non-2xx is thrown as a `BootstrapFetchError` carrying the HTTP status.

import type { DropPasteProfile } from "./fileDrop"
import type { FlatSortKey } from "./flatList"
import type { MacroView } from "./types"

// The bootstrap document; field names and types mirror the server's snake_case
// JSON. An optional field may be absent against an older server, and consumers
// fall back to the default documented on it rather than assuming it is present.
export interface Bootstrap {
  /** Configured agent providers (the new-agent / change-provider pickers). */
  available_providers: string[]
  /** What config says a dropped file's path should look like per provider: the
   * paste form and the file name of the command the block runs. Keyed by provider.
   *
   * The fallback only, for a pane with no live process; what a live process
   * launched with rides `AgentTabView.drop_paste`. An absent provider or field
   * resolves to "bare" with no length limit (see `dragDropPasteFormFor`). */
  provider_drop_paste?: Record<string, DropPasteProfile>
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
  /** Raw `config.ui.github_integration` flag, distinct from the `gh_available`
   * composite: false means integration is off, not merely that `gh` is unreachable. */
  github_integration: boolean
  /** Mirrors `config.ui.copy_on_select`: whether selecting text in the web
   * terminal auto-copies it to the clipboard (default true). */
  copy_on_select: boolean
  /** Mirrors `config.ui.terminal_font_family`: a font on the viewing device, placed
   * ahead of the bundled stack, which still fills in glyphs it lacks. Falls back to
   * "", the bundled stack alone. */
  terminal_font_family?: string
  /** Mirrors `config.ui.terminal_font_size`: the web terminal's font size in pixels,
   * valid 8..=32, falling back to 14 when absent or out of range. */
  terminal_font_size?: number
  /** Mirrors `config.ui.compose_bar`: when the terminal shows the compose bar and
   * redirects a tap into it. With the bar down, a tap focuses xterm directly.
   *
   * One of "auto" (the browser decides from the primary pointer), "always" or
   * "never"; an absent or unrecognized value reads as "auto" via `composeBarMode`. */
  compose_bar?: string
  /** Mirrors `config.ui.mobile_accessory_bar`: whether the touch terminal screens
   * show the accessory key bar. A pure render gate, restored from the input menu or
   * Preferences; falls back to true. */
  mobile_accessory_bar?: boolean
  /** Mirrors `config.ui.upload_write_gitignore`: whether the agent upload directory
   * keeps a `.gitignore` of `*`, so a dropped file stays invisible to git. Falls
   * back to true. Its `ui.upload_directory` companion is deliberately not a
   * preference: it is a path, and there is no directory picker for it. */
  upload_write_gitignore?: boolean
  /** Mirrors `config.ui.upload_pasted_text_chars`: how long a text paste onto an
   * agent pane may run before dux saves it as a file and pastes the path; 0 is off.
   * Absent reads as off, since the document arrives after the first render. Never
   * applies to a terminal pane, where a long paste is a command or a heredoc. */
  upload_pasted_text_chars?: number
  /** Mirrors `config.ui.auto_reopen_agents`: the global startup auto-reopen switch,
   * which relaunches agents that were running at exit and carry the per-agent
   * opt-in. Falls back to false, the config default. */
  auto_reopen_agents?: boolean
  /** Mirrors `config.ui.attention_grace_seconds`: seconds the attention indicators
   * survive the tab returning to the foreground; 0 clears immediately, absent is 3. */
  attention_grace_seconds?: number
  /** Mirrors `config.capabilities.web_notifications`: whether an agent's
   * notification sequences reach a browser Notification, still gated on visitor
   * permission and a backgrounded tab. Falls back to true. */
  web_notifications?: boolean
  /** Mirrors `config.capabilities.hyperlinks`: whether the web terminal renders OSC 8
   * hyperlinks as clickable (http/https only). Falls back to true. */
  hyperlinks?: boolean
  /** Mirrors `config.capabilities.clipboard_passthrough`, normalized: whether an
   * agent's OSC 52 clipboard set reaches the browser clipboard. "focused"/"always"
   * write it, "off" never does, and the `capabilities.passthrough` master switch is
   * already resolved into this value, so there is no second field. Absent is
   * "focused". */
  clipboard_passthrough?: "focused" | "always" | "off"
  /** Mirrors `config.ui.pr_banner_position`: "bottom" places the PR lane below the
   * terminal, anything else above. The server sends a free string. */
  pr_banner_position: "top" | "bottom"
  /** Mirrors `config.ui.agent_sort`: the agent-list sort mode, persisted server-side
   * so every client agrees. Falls back to "active". */
  agent_sort?: FlatSortKey
  /** Mirrors `config.ui.agent_scrollback_lines`; sizes each xterm.js instance. */
  agent_scrollback_lines: number
  /** Mirrors `config.ui.show_changes_pane`; the desktop Changes-pane default. */
  show_changes_pane: boolean
  /** Mirrors `[server] tailscale` as its canonical name: "auto" | "yes" | "no".
   * An older server omits it, so the Preferences row falls back to "auto". */
  tailscale_mode?: string
  /** True when this run of the server was started with `--no-tailscale`, which
   * outranks the saved mode until it restarts. Per run, not projected from config. */
  tailscale_forced_no?: boolean
  /** Mirrors `config.ui.always_show_tab_strip`: when true the agent tab strip renders
   * even with a single tab. Default false. */
  always_show_tab_strip: boolean
  /** Mirrors `config.ui.tab_reaches_agent`: when true the terminal UI's typeable
   * pane sends Tab and Shift-Tab to the agent instead of cycling panes. Nothing in
   * the web UI acts on it; Preferences shows and changes it. Falls back to false. */
  tab_reaches_agent?: boolean
  /** Global environment variables applied to every spawned agent/terminal. */
  global_env: Record<string, string>
  /** Mirrors `config.ui.status_clear_seconds`: the base window every tone is scaled
   * off in `lib/notify.ts`, not info/success alone. 0 never auto-clears a final
   * state; absent is 6. */
  status_clear_seconds: number
  /** The operator-chosen display name for this instance (`config.server.title`),
   * shown as the tab title and wordmark. A missing or blank value resolves to "dux"
   * via `resolveInstanceTitle`. */
  title?: string
  /** The operator-chosen favicon (`config.server.favicon`), resolved and applied by
   * `applyFavicon`: empty shows the bundled duck, a curated tint name recolours its
   * silhouette, and anything else degrades to the duck with a one-time notice. */
  favicon?: string
  /** Mirrors `config.ui.agent_tabs_max`, normalized: the per-agent tab cap, counting
   * the session-slot tab. The strip disables its "+" at the cap and the server
   * re-enforces on create; absent falls back to `DEFAULT_AGENT_TABS_MAX`. */
  agent_tabs_max?: number
  /** Mirrors `config.ui.attention_indicator`: whether any cue is shown when an agent
   * asks for attention. Falls back to true. */
  attention_indicator?: boolean
  /** Mirrors `config.ui.attention_on_bell`: whether a plain terminal bell also counts
   * as an attention request. No effect while `attention_indicator` is false; falls
   * back to true. */
  attention_on_bell?: boolean
  /** Mirrors `config.defaults.provider`: the global default provider for new agents
   * in projects with no override of their own (`ProjectView.default_provider` in
   * `types.ts` is the effective per-project value). Falls back to "claude". */
  global_default_provider?: string
  /** The welcome screen's copy, from `dux_core::welcome_screen` so both surfaces say
   * identical words. Present unconditionally, since the app menu can open the screen
   * on demand, but consumers must tolerate `undefined`. Not `welcome_tips`. */
  welcome_screen?: WelcomeScreenView
  /** `dux_core::urls::WEBSITE`, where the welcome screen's secondary button goes.
   * Server-projected so the two surfaces cannot disagree about a dux URL. */
  website_url?: string
  /** The first-load screen this launch should show, or absent for neither. Decided
   * once at startup and held in the server's memory, so a browser connecting at any
   * point still receives it; `dismissFirstLoad` settles it on both surfaces. */
  pending_first_load?: PendingFirstLoad | null
  /** Mirrors `config.ui.disable_automated_welcome_screen`: suppresses the automatic
   * first-run welcome only, never the app menu entry. Falls back to false. */
  disable_automated_welcome_screen?: boolean
  /** Mirrors `config.ui.disable_release_notes`: suppresses the automatic what's-new
   * screen only, never the app menu entry. Falls back to false. */
  disable_release_notes?: boolean
  /** Mirrors `config.server.file_drop_max_bytes`: the per-file cap for a dropped
   * file, where 0 switches file drop off and the pane offers no drop target at all.
   * The server refusal remains the real enforcement, and an absent value reads as
   * off, since this document arrives after the first render. */
  file_drop_max_bytes?: number
  /** Mirrors `config.server.replay_wait_seconds`: seconds of visible time a terminal
   * pane waits for its screen after connecting, before offering Reconnect. */
  replay_wait_seconds?: number
  /** Mirrors `config.server.reconnect_backoff_cap_seconds`: the longest gap
   * between two automatic reconnect attempts. */
  reconnect_backoff_cap_seconds?: number
  /** Mirrors `config.server.heartbeat_seconds`: how often a visible page checks
   * its terminal connection is really alive. */
  heartbeat_seconds?: number
  /** Mirrors `config.server.heartbeat_deadline_seconds`: seconds of VISIBLE time
   * to wait for that answer before forcing a plain reconnect. */
  heartbeat_deadline_seconds?: number
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

/** Fallback per-agent tab cap when the server omits `agent_tabs_max`. A duplicated
 * literal that must stay equal to `dux_core::config::DEFAULT_AGENT_TABS_MAX` in
 * `crates/dux-core/src/config.rs`; bump one and bump the other. */
export const DEFAULT_AGENT_TABS_MAX = 20

// A failed bootstrap fetch; `status` is 0 for a transport failure with no response.
// The boot path swallows it and keeps the last-known bootstrap, retrying on a later
// `config.changed` event or reconnect.
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
