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

import type { MacroView, PaletteCommandView } from "./types"

// The bootstrap document. Field names/types mirror the server's JSON (snake_case)
// and the values the legacy ViewModel carried, so consumers move over without a
// shape change. Newer fields may be absent when talking to an older server (a `?`
// marks the ones typed optional, e.g. `title`); consumers fall back to the
// per-field documented default rather than assuming every field is present.
export interface Bootstrap {
  /** Configured agent providers (the new-agent / change-provider pickers). */
  available_providers: string[]
  /** Text macros from `[macros]` in config order (the macro popover/editor). */
  macros: MacroView[]
  /** Surface-aware command-palette commands (Web/Both subset), in registry order. */
  palette_commands: PaletteCommandView[]
  /** The rotating welcome tips shown on the empty-state screen. */
  welcome_tips: string[]
  /** The binary's display version ('vX.Y.Z' or 'development'); shown in the sidebar. */
  dux_version: string
  /** Whether the new-agent name dialog pre-checks "Use randomized pet name". */
  randomize_agent_names_by_default: boolean
  /** Whether the new-agent-from-PR flow is available (GitHub integration + `gh`). */
  gh_available: boolean
  /** Mirrors `config.ui.pr_banner_position`: "bottom" places the PR lane below
   * the terminal, anything else above. (Server sends a free string; the two
   * known values are the only ones the UI branches on.) */
  pr_banner_position: "top" | "bottom"
  /** Mirrors `config.ui.agent_scrollback_lines`; sizes each xterm.js instance. */
  agent_scrollback_lines: number
  /** Mirrors `config.ui.show_changes_pane`; the desktop Changes-pane default. */
  show_changes_pane: boolean
  /** Global environment variables applied to every spawned agent/terminal. */
  global_env: Record<string, string>
  /** Mirrors `config.ui.status_clear_seconds`: how long an info/success toast
   * stays before auto-clearing. 0 means "never auto-clear" (sticky like a
   * warning/error). The web computes its info-toast duration from this; older
   * servers omit it, so consumers fall back to 6. */
  status_clear_seconds: number
  /** The operator-chosen display name for this dux instance (`config.server
   * .title`). Shown as the browser tab title and the projects-pane wordmark.
   * Optional: older servers omit it, so consumers resolve a missing/blank value
   * to "dux" via `resolveInstanceTitle`. */
  title?: string
}

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
