// THE LIVE-VALUES CONTAINER for the terminal pane.
//
// The pane's lifecycle effect owns a terminal and a socket and must re-run only
// when the streamed target changes, so every closure it creates outlives the
// render that created it. Everything those closures need to read has to reach
// them some other way, and there used to be sixteen individual ref mirrors and
// sixteen effects doing exactly that, one per preference, each with the same
// three-line shape and its own chance to be forgotten.
//
// This replaces all of them with ONE container and ONE synchronising effect,
// and it draws the line the mirrors never did:
//
//   READ-ONLY SETTINGS (this file) travel one way. The render computes them,
//   the container publishes them, and the long-lived closures read them AT CALL
//   TIME. Nothing inside the lifecycle ever writes one. A value that only has
//   to be fresh belongs here and needs no ceremony beyond a field.
//
//   WRITE-BACK CHANNELS (see `channels.ts`) travel both ways, are named, and
//   have exactly one declared owner each. There are three of them, they are
//   the state the machines mutate mid-gesture and the screen renders, and they
//   are deliberately NOT fields here: a value the wiring writes is a different
//   thing from a preference it reads, and collapsing the two is how the mirrors
//   became indistinguishable from smuggled state in the first place.
//
// The container's own freshness contract: the snapshot is published in a
// LAYOUT effect with no dependency list, so it lands in EVERY commit's layout
// phase, before the pane's relayout (declared after this hook, so ordered
// after it) and before every passive effect. The old passive publish was one
// phase too late for exactly one reader: the relayout is itself a layout
// effect, and a live preference flip it acted on (the watcher-view mode) read
// the PREVIOUS commit's snapshot through the coordinator, shrinking the font
// for a view whose grid it then refused to adopt. Every other reader is an
// event-time closure (socket callbacks, gestures, timers), all of which run
// after layout anyway, so publishing earlier only narrows their stale window.
// The initial value is the mount render's snapshot, so no effect ever reads an
// unset field.
import { useLayoutEffect, useRef } from "react"

import type { AgentTabView } from "@/lib/types"
import type { ConfiguredDropPaste, DropPasteProfile } from "@/lib/fileDrop"

/// Everything the pane's long-lived closures may read, and nothing they write.
///
/// Every field is a value the RENDER already computed: a preference off the
/// bootstrap document, a name off the spine, or a derived flag. If a new
/// closure needs a new preference, it becomes a field here and reads
/// `live.current.thing` at the moment it needs it; there is no second step.
export type TerminalLiveSettings = {
  /// `agent_scrollback_lines`, read lazily on every (re)connect so xterm's
  /// 1000-line default never trims the reconnect replay.
  scrollbackLines: number
  /// `ui.copy_on_select`, read inside the mouseup and touch-lift handlers.
  copyOnSelect: boolean
  /// The two `ui.terminal_font_*` settings, RAW (unsanitised, unclamped): the
  /// terminal's construction and the live-apply effect both resolve them
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
  /// `ui.watcher_view` resolved to a boolean: whether a NON-OWNER renders at
  /// the PTY's own grid (shrinking the font to fit) rather than fitting this
  /// container. Read by the resize coordinator's `viewerMode`, which ANDs it
  /// with the live ownership verdict. The layout-phase publish above is what
  /// keeps it in step with the relayout: a preference flip must be visible to
  /// the coordinator in the SAME commit the relayout acts on it, or the flip
  /// shrinks the font without ever adopting the grid.
  watcherFaithful: boolean
  /// Whether the faithful watcher is OVERFLOWING on purpose: even the floor
  /// font could not fit the adopted grid, so the terminal stands at its true
  /// size and the host scrolls to the rest of it. Read by the touch gesture's
  /// `scrollAllowed`, which leaves vertical drags to the browser while the
  /// host is the scroller. Always false for an owner and in every non-overflow
  /// state, so those paths never even look at it.
  viewerOverflow: boolean
  /// Whether the compose bar is the typing surface. Deliberately a MIRROR that
  /// lags the rendered value by one commit, and both mismatch directions
  /// degrade gracefully: a stale `false` falls through to `term.focus()`, a
  /// stale `true` at worst redirects one tap into a bar that just unmounted
  /// (the focus call no-ops on a null ref).
  composeActive: boolean
}

/// The read side of the container. A ref rather than a getter so a call site
/// reads one field without paying for a snapshot, and readonly so the
/// one-way-ness is a type error to break rather than a convention.
export type LiveSettings = { readonly current: TerminalLiveSettings }

/// Publish this render's settings for the lifecycle's closures to read.
///
/// Call it BEFORE the lifecycle hook in the component body: effects run in
/// declaration order, so this ordering is what guarantees a lifecycle effect
/// re-running for a new target reads the new target's settings rather than the
/// previous one's.
export function useTerminalLiveSettings(
  values: TerminalLiveSettings,
): LiveSettings {
  const ref = useRef(values)
  // No dependency list on purpose: this is sixteen effects' worth of
  // synchronisation, and enumerating the fields here would reintroduce exactly
  // the per-field bookkeeping the container exists to delete. Writing a ref is
  // not a render effect, so running it on every commit costs one assignment.
  // A LAYOUT effect, not a passive one: the pane's relayout is a layout effect
  // that reaches this container through the coordinator's `viewerMode`, and it
  // must read THIS commit's snapshot (see the module doc).
  useLayoutEffect(() => {
    ref.current = values
  })
  return ref
}
