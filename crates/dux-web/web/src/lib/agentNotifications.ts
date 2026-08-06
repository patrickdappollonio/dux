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

/** The clipboard passthrough mode, mirroring the Rust
 * `capabilities.clipboard_passthrough`: `off` never writes the browser clipboard;
 * `focused`/`always` write it (the browser itself additionally requires the tab to
 * have focus, so on the web `always` behaves like `focused`). */
export type ClipboardPassthroughMode = "focused" | "always" | "off"

/** Minimum gap between fired desktop notifications; a repeat inside the window is
 * suppressed so an agent that spams OSC 9 cannot stack a wall of notifications. */
export const NOTIFY_MIN_INTERVAL_MS = 1000
/** Minimum gap between browser-clipboard writes. Unlike notifications this is
 * keep-last: a write suppressed inside the window is deferred and applied when the
 * window expires, so the final clipboard value is never dropped. */
export const CLIPBOARD_MIN_INTERVAL_MS = 500

/** A leading-edge throttle decision: fire when at least `intervalMs` has elapsed
 * since `lastAt`. Pure and unit-tested; the caller owns the `lastAt` clock. */
export function leadingEdgeAllowed(
  lastAt: number,
  now: number,
  intervalMs: number,
): boolean {
  return now - lastAt >= intervalMs
}

export interface AgentNotificationOptions {
  /** Live read of the `web_notifications` config bit. */
  enabled: () => boolean
  /** Live read of `capabilities.passthrough`, the master switch over everything
   * an agent forwards outward. False suppresses BOTH the notification and the
   * clipboard write here, whatever `enabled` and `clipboardMode` say, which is
   * what makes the switch mean the same thing on this surface as it does on the
   * TUI host forward. Defaults to true when the caller omits it (older
   * bootstrap), matching the field's own older-server fallback. */
  passthrough?: () => boolean
  /** Title shown on the desktop notification (e.g. the agent's name). */
  title: () => string
  /** Live read of `capabilities.clipboard_passthrough`. Defaults to "focused"
   * when the caller omits it (older bootstrap). */
  clipboardMode?: () => ClipboardPassthroughMode
  /** A stable per-session/tab id used as the Notification `tag` so a repeat from
   * the same agent REPLACES the previous one instead of stacking. */
  tag?: () => string
}

/** Register the OSC handlers on a viewer terminal. Returns a disposer that removes
 * every handler. Notifications only fire when {@link shouldFireNotification} allows
 * it; clipboard writes only when the document has focus (a background tab writing
 * the visitor's clipboard would be surprising). */
export function registerAgentNotifications(
  term: Terminal,
  opts: AgentNotificationOptions,
): () => void {
  // Leading-edge throttle clock for fired notifications (closure-local so two
  // panes never interfere).
  let lastNotifyAt = Number.NEGATIVE_INFINITY
  // The master switch. Absent means on, so an older bootstrap that never learned
  // to publish it behaves exactly as it did before.
  const masterOn = (): boolean => opts.passthrough?.() ?? true
  const fire = (title: string, body: string) => {
    if (!masterOn()) return
    if (typeof Notification === "undefined") return
    const ok = shouldFireNotification({
      enabled: opts.enabled(),
      permission: Notification.permission,
      hidden: typeof document !== "undefined" && document.hidden,
      hasFocus: typeof document !== "undefined" && document.hasFocus(),
    })
    if (!ok) return
    const now = Date.now()
    if (!leadingEdgeAllowed(lastNotifyAt, now, NOTIFY_MIN_INTERVAL_MS)) return
    lastNotifyAt = now
    try {
      // A stable `tag` makes a repeat from the same agent REPLACE the previous
      // notification instead of stacking a fresh one.
      const tag = opts.tag?.()
      new Notification(title, tag ? { body, tag } : { body })
    } catch {
      // Constructing a Notification can throw on some platforms; ignore.
    }
  }

  // Keep-last clipboard throttle: a write suppressed inside the interval is
  // deferred and flushed when the window expires so the final value is never lost.
  let lastClipboardAt = Number.NEGATIVE_INFINITY
  let clipboardTimer: ReturnType<typeof setTimeout> | null = null
  let pendingClipboard: string | null = null
  const doClipboardWrite = (text: string) => {
    // Re-check the runtime gate at write time (focus can change while a deferred
    // write waits). The browser only permits a clipboard write from a focused tab.
    if (
      typeof document !== "undefined" &&
      document.hasFocus() &&
      typeof navigator !== "undefined" &&
      navigator.clipboard
    ) {
      void navigator.clipboard.writeText(text).catch(() => {})
    }
  }
  const writeClipboard = (text: string) => {
    const now = Date.now()
    if (leadingEdgeAllowed(lastClipboardAt, now, CLIPBOARD_MIN_INTERVAL_MS)) {
      lastClipboardAt = now
      doClipboardWrite(text)
      return
    }
    // Inside the window: remember the latest value and (if not already) schedule
    // the trailing flush.
    pendingClipboard = text
    if (clipboardTimer === null) {
      const wait = CLIPBOARD_MIN_INTERVAL_MS - (now - lastClipboardAt)
      clipboardTimer = setTimeout(
        () => {
          clipboardTimer = null
          if (pendingClipboard !== null) {
            lastClipboardAt = Date.now()
            const text = pendingClipboard
            pendingClipboard = null
            doClipboardWrite(text)
          }
        },
        Math.max(0, wait),
      )
    }
  }

  const clipboardMode = (): ClipboardPassthroughMode =>
    opts.clipboardMode?.() ?? "focused"

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
      // "off" consumes the sequence but never writes; "focused"/"always" write
      // (subject to the focus + throttle gates in writeClipboard). The master
      // switch is already folded into the published mode, so it would be enough
      // on its own; check it here too so this handler does not depend on that
      // folding staying in place server-side.
      if (text !== null && masterOn() && clipboardMode() !== "off") {
        writeClipboard(text)
      }
      // Consume so xterm never writes the HOST clipboard or answers a read query.
      return true
    }),
  ]

  return () => {
    if (clipboardTimer !== null) {
      clearTimeout(clipboardTimer)
      clipboardTimer = null
    }
    for (const d of disposers) d.dispose()
  }
}
