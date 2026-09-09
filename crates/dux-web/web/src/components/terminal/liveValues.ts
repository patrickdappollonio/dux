// The live-values container: the one place the lifecycle's long-lived closures
// read a render-computed setting from, at call time. Settings here travel ONE WAY,
// and anything the wiring writes is a named channel in `channels.ts` instead. The
// snapshot is published in a LAYOUT effect, so the pane's relayout, itself a layout
// effect, does not act on the previous commit's.
import { useLayoutEffect, useRef } from "react"

import type { AgentTabView } from "@/lib/types"
import type { ConfiguredDropPaste, DropPasteProfile } from "@/lib/fileDrop"

/// Everything the pane's long-lived closures may read, and nothing they write:
/// every field is a value the render already computed.
export type TerminalLiveSettings = {
  /// `agent_scrollback_lines`, read lazily on every (re)connect so xterm's
  /// 1000-line default never trims the reconnect replay.
  scrollbackLines: number
  /// `ui.copy_on_select`, read inside the mouseup and touch-lift handlers.
  copyOnSelect: boolean
  /// The two `ui.terminal_font_*` settings, RAW: every reader resolves them
  /// through `terminalFont.ts`, which is the one place that knows the rules.
  fontFamily: string
  fontSize: number
  /// Whether uploads are available at all (`[server] file_drop_max_bytes > 0`).
  /// Not-yet-known reads as OFF; see the pane's own note on why.
  fileDropEnabled: boolean
  /// `ui.upload_pasted_text_chars`, 0 when off or unpublished.
  pastedTextChars: number
  /// `ui.attention_grace_seconds`, already in milliseconds.
  attentionGraceMs: number
  /// `capabilities.web_notifications`.
  webNotifications: boolean
  /// `capabilities.hyperlinks`, read by the OSC 8 parser gate and the link
  /// machine's own truth table.
  hyperlinks: boolean
  /// `capabilities.clipboard_passthrough`, already resolved server-side against
  /// the passthrough master switch.
  clipboardPassthrough: "focused" | "always" | "off"
  /// The title a bridged desktop notification carries, matched exhaustively on
  /// the owner by the render (never rebuilt from the nullable id pair here).
  notifyTitle: string
  /// The focused tab's provider name, for the launch spinner's wording and the
  /// configured drop-paste lookup.
  providerName: string | undefined
  /// `[providers.*] drop_paste`, off the bootstrap document.
  configuredDropPaste: ConfiguredDropPaste
  /// What the focused tab's LIVE process launched with, off the spine.
  /// `undefined` for a dormant tab and for every terminal.
  launchedDropPaste: DropPasteProfile | undefined
  /// The owning session's tabs, for the tab-gone check. A dependency of the
  /// lifecycle effect would rebuild the socket on every spine refresh.
  sessionTabs: AgentTabView[] | undefined
  /// Whether the faithful watcher is OVERFLOWING on purpose: the floor font could
  /// not fit the adopted grid, so the host scrolls. Always false for an owner.
  viewerOverflow: boolean
  /// Whether the compose bar is the typing surface. A mirror that lags the render
  /// by one commit; both mismatch directions degrade to a harmless focus call.
  composeActive: boolean
}

/// The read side of the container: a ref, so a call site reads one field without
/// a snapshot, and readonly, so the one-way-ness is a type error to break.
export type LiveSettings = { readonly current: TerminalLiveSettings }

/// Publish this render's settings for the lifecycle's closures. Call it BEFORE
/// the lifecycle hook, or an effect re-running for a new target reads stale ones.
export function useTerminalLiveSettings(
  values: TerminalLiveSettings,
): LiveSettings {
  const ref = useRef(values)
  // No dependency list on purpose: enumerating the fields is the per-field
  // bookkeeping this container exists to delete, and a ref write costs one
  // assignment. Layout, not passive, so the relayout reads THIS commit's snapshot.
  useLayoutEffect(() => {
    ref.current = values
  })
  return ref
}
