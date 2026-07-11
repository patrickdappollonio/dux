// Bridge an agent's terminal notification and clipboard escape sequences to the
// browser, mirroring the TUI's host passthrough. The web terminal's xterm.js is a
// VIEWER of a PTY that dux-core already drives (see suppressViewerReports), so we
// hook the same OSC codes the core scanner whitelists and turn them into a browser
// Notification (OSC 9 / 99 / 777) or a clipboard write (OSC 52 SET).
//
// The parsing rules below intentionally match the pure Rust scanner in
// `crates/dux-core/src/attention.rs` so both surfaces agree on what counts as a
// notification. They are exported and unit-tested independently of xterm.
import type { Terminal } from "@xterm/xterm"

/** Whether an OSC 9 payload (the text after `9;`) is a progress report rather than
 * a notification. Progress is `4;<state>;…` where `<state>` is a 1-2 digit token;
 * anything else is a notification. Matches the Rust `is_progress_state` rule. */
export function osc9IsProgress(data: string): boolean {
  if (!data.startsWith("4;") && data !== "4") return false
  const parts = data.split(";")
  if (parts[0] !== "4") return false
  const state = parts[1]
  return state !== undefined && /^\d{1,2}$/.test(state)
}

/** The body text of an OSC 9 notification (everything after `9;`). Returns null
 * for a progress report or an empty body. */
export function osc9NotifyBody(data: string): string | null {
  if (data.length === 0 || osc9IsProgress(data)) return null
  return data
}

/** Parse an OSC 777 payload. A notification is `notify;<title>;<body>`; returns the
 * title/body, or null when it is not a notify. */
export function osc777Notify(
  data: string,
): { title: string; body: string } | null {
  if (!data.startsWith("notify")) return null
  const parts = data.split(";")
  // parts[0] === "notify"; title and body follow.
  const title = parts[1] ?? ""
  const body = parts.slice(2).join(";")
  return { title, body }
}

/** Parse an OSC 99 (kitty notification protocol) payload `<metadata>;<body>`.
 * Fires only for a final (`d` absent/=1), displayable (`p` absent/title/body)
 * notification, never for a `p=?` query. Matches the Rust rule. */
export function osc99Notify(data: string): { body: string } | null {
  const semi = data.indexOf(";")
  const metadata = semi === -1 ? data : data.slice(0, semi)
  const body = semi === -1 ? "" : data.slice(semi + 1)
  let dFinal = true
  let pOk = true
  for (const token of metadata.split(":")) {
    const eq = token.indexOf("=")
    const key = eq === -1 ? token : token.slice(0, eq)
    const value = eq === -1 ? undefined : token.slice(eq + 1)
    if (key === "d") dFinal = value !== "0"
    else if (key === "p") pOk = value === undefined || value === "title" || value === "body"
  }
  if (!dFinal || !pOk) return null
  return { body }
}

/** Parse an OSC 52 clipboard payload `<selection>;<data>`. Returns the text for a
 * SET (data !== "?"), or null for a read query or a malformed payload. */
export function osc52SetText(data: string): string | null {
  const semi = data.indexOf(";")
  if (semi === -1) return null
  const encoded = data.slice(semi + 1)
  if (encoded === "?" || encoded.length === 0) return null
  return decodeBase64Utf8(encoded)
}

function decodeBase64Utf8(b64: string): string | null {
  try {
    const binary = atob(b64)
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0))
    return new TextDecoder().decode(bytes)
  } catch {
    return null
  }
}

/** The runtime gate for firing a browser notification: the config bit is on,
 * permission is granted, and the tab is currently backgrounded (hidden or
 * unfocused) so we never nag while the user is already looking. */
export function shouldFireNotification(ctx: {
  enabled: boolean
  permission: NotificationPermission
  hidden: boolean
  hasFocus: boolean
}): boolean {
  return (
    ctx.enabled &&
    ctx.permission === "granted" &&
    (ctx.hidden || !ctx.hasFocus)
  )
}

export interface AgentNotificationOptions {
  /** Live read of the `web_notifications` config bit. */
  enabled: () => boolean
  /** Title shown on the desktop notification (e.g. the agent's name). */
  title: () => string
}

/** Register the OSC handlers on a viewer terminal. Returns a disposer that removes
 * every handler. Notifications only fire when {@link shouldFireNotification} allows
 * it; clipboard writes only when the document has focus (a background tab writing
 * the visitor's clipboard would be surprising). */
export function registerAgentNotifications(
  term: Terminal,
  opts: AgentNotificationOptions,
): () => void {
  const fire = (title: string, body: string) => {
    if (typeof Notification === "undefined") return
    const ok = shouldFireNotification({
      enabled: opts.enabled(),
      permission: Notification.permission,
      hidden: typeof document !== "undefined" && document.hidden,
      hasFocus: typeof document !== "undefined" && document.hasFocus(),
    })
    if (!ok) return
    try {
      new Notification(title, { body })
    } catch {
      // Constructing a Notification can throw on some platforms; ignore.
    }
  }

  const disposers = [
    term.parser.registerOscHandler(9, (data) => {
      if (osc9IsProgress(data)) return false
      const body = osc9NotifyBody(data)
      if (body !== null) fire(opts.title(), body)
      return true
    }),
    term.parser.registerOscHandler(99, (data) => {
      const parsed = osc99Notify(data)
      if (parsed) fire(opts.title(), parsed.body)
      // Consume every OSC 99 (including continuations/close/queries) so the viewer
      // xterm never mishandles the kitty protocol.
      return true
    }),
    term.parser.registerOscHandler(777, (data) => {
      const parsed = osc777Notify(data)
      if (!parsed) return false
      fire(parsed.title || opts.title(), parsed.body)
      return true
    }),
    term.parser.registerOscHandler(52, (data) => {
      const text = osc52SetText(data)
      if (
        text !== null &&
        typeof document !== "undefined" &&
        document.hasFocus() &&
        typeof navigator !== "undefined" &&
        navigator.clipboard
      ) {
        void navigator.clipboard.writeText(text).catch(() => {})
      }
      // Consume so xterm never writes the HOST clipboard or answers a read query.
      return true
    }),
  ]

  return () => {
    for (const d of disposers) d.dispose()
  }
}
