// Turn an agent's OSC sequences into a browser Notification (9 / 99 / 777) or a
// clipboard write (52 SET). The parsing rules must match the Rust scanner in
// `crates/dux-core/src/attention.rs` so both surfaces agree on what is a notification.
import type { Terminal } from "@xterm/xterm"

/** Whether an OSC 9 payload (the text after `9;`) is a progress report rather than a
 * notification: `4;<state>` with a 1-2 digit state. Matches Rust `is_progress_state`. */
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
  const title = parts[1] ?? ""
  const body = parts.slice(2).join(";")
  return { title, body }
}

/** Parse an OSC 99 (kitty) payload `<metadata>;<body>`. Returns a body only for a final
 * (`d` absent/=1), displayable (`p` absent/title/body) notification, never a `p=?` query. */
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

/** The runtime gate for firing a browser notification: enabled, permission granted, and
 * the tab backgrounded (hidden or unfocused) so it never nags while the user is looking. */
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

/** Mirrors the Rust `capabilities.clipboard_passthrough`: `off` never writes the browser
 * clipboard. On the web `always` behaves like `focused`; a write needs a focused tab. */
export type ClipboardPassthroughMode = "focused" | "always" | "off"

/** Minimum gap between fired desktop notifications; a repeat inside the window is
 * suppressed so an agent that spams OSC 9 cannot stack a wall of notifications. */
export const NOTIFY_MIN_INTERVAL_MS = 1000
/** Minimum gap between browser-clipboard writes. Keep-last: a write suppressed inside
 * the window is deferred to its expiry, so the final clipboard value is never dropped. */
export const CLIPBOARD_MIN_INTERVAL_MS = 500

/** A leading-edge throttle decision: fire when at least `intervalMs` has elapsed
 * since `lastAt`. The caller owns the `lastAt` clock. */
export function leadingEdgeAllowed(
  lastAt: number,
  now: number,
  intervalMs: number,
): boolean {
  return now - lastAt >= intervalMs
}

export interface AgentNotificationOptions {
  /** Live read of the `web_notifications` config bit, the only switch over desktop
   * notifications: `capabilities.passthrough` deliberately does not gate them. */
  enabled: () => boolean
  /** Title shown on the desktop notification (e.g. the agent's name). */
  title: () => string
  /** Live read of `capabilities.clipboard_passthrough`, into which the server has already
   * resolved the `capabilities.passthrough` master switch. Defaults to "focused". */
  clipboardMode?: () => ClipboardPassthroughMode
  /** A stable per-session/tab id used as the Notification `tag` so a repeat from
   * the same agent replaces the previous one instead of stacking. */
  tag?: () => string
}

/** Register the OSC handlers on a viewer terminal; the returned disposer removes them all.
 * Notifications fire under {@link shouldFireNotification}, clipboard writes only when focused. */
export function registerAgentNotifications(
  term: Terminal,
  opts: AgentNotificationOptions,
): () => void {
  // Closure-local so two panes never share a throttle clock.
  let lastNotifyAt = Number.NEGATIVE_INFINITY
  const fire = (title: string, body: string) => {
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
      const tag = opts.tag?.()
      new Notification(title, tag ? { body, tag } : { body })
    } catch {
      // Constructing a Notification can throw on some platforms; ignore.
    }
  }

  let lastClipboardAt = Number.NEGATIVE_INFINITY
  let clipboardTimer: ReturnType<typeof setTimeout> | null = null
  let pendingClipboard: string | null = null
  const doClipboardWrite = (text: string) => {
    // Focus can change while a deferred write waits, and only a focused tab may write.
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
      // Consume every OSC 99, continuations and queries included, so the viewer
      // xterm never answers the kitty protocol itself.
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
      // "off" consumes the sequence without writing. The `capabilities.passthrough`
      // master switch is already resolved into this mode, so nothing else is checked.
      if (text !== null && clipboardMode() !== "off") {
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
