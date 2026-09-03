import { useSyncExternalStore } from "react"

// WHAT ONE PANE'S INPUT GROUP WOULD SAY, keyed by PTY id, for whichever TOP
// menu is on screen above it.
//
// The pane is the only thing that knows the answers: whether it owns the input,
// whether uploads are switched on, whether the virtual input is up and whether
// its key row is. The menus that have to OFFER those answers are all somewhere
// else (the phone's flap, the desktop pane header's `⋯`, the sidebar row's and
// the floating pill's, which are four anchors on one body), and none of them is
// inside the pane. So the pane publishes and the menus read.
//
// This used to be a narrower thing: the items the input `⋯` would have shown if
// it had had a row to sit in, published only in the one state that took its row
// away (theater on a computer). "Type directly in the terminal" now removes the
// whole bottom bar, so that state is the ordinary one, and the INPUT group in
// the top menu is where the way back lives on every surface.
//
// The pane publishes only while it OWNS the input, so a mounted viewer pane
// cannot shadow a mounted owner pane's answers, and a group with nothing in it
// renders no label and no separator.
//
// Keyed rather than a single slot, on the `attachRegistry` precedent: several
// panes can be mounted at once, and a menu must read the one it belongs to
// rather than whichever mounted last. WHICH key that is comes from the anchor,
// the only thing that knows what is on screen under it: a companion terminal's
// pane publishes under the terminal's id while the menu over it is its agent's,
// so a scan derived from the menu's subject would find nothing at all.

/// The INPUT group's items for one pane, as gates rather than as rendering.
///
/// "Attach a file…" is deliberately NOT here: the item and the act behind it
/// are the same fact, and `attachRegistry` already publishes it under the same
/// pty id on exactly the same condition. Asking two registries the same
/// question is how the two would eventually disagree.
export interface PaneInputGroupGates {
  /// "Use virtual input", the way back from typing straight into the terminal.
  /// Present only while the virtual input is DOWN: absent, never disabled,
  /// while it is already up, because the bottom `⋯` owns the other direction.
  surfaceSwitch: boolean
  /// "Show terminal keys". Same shape as `surfaceSwitch`: the top menu carries
  /// the keys item only while there is no bottom bar to carry it, so the two
  /// menus can never offer the same row.
  keysToggle: boolean
}

const groups = new Map<string, PaneInputGroupGates>()
const listeners = new Set<() => void>()
// A monotonic counter IS the snapshot, for the same reason `attachRegistry`
// uses one: `useSyncExternalStore` compares snapshots by value.
let version = 0

function publish(): void {
  version++
  for (const listener of listeners) listener()
}

/**
 * Publish this pane's input group. Returns the retirement.
 *
 * LAST WRITE WINS, and the retirement removes the entry only while it is still
 * the one this call installed, so a replacement pane's registration survives
 * the outgoing pane's late cleanup. Same guard, and same reason, as
 * `registerAttachCapability`.
 */
export function registerPaneInputGroup(
  ptyId: string,
  gates: PaneInputGroupGates,
): () => void {
  groups.set(ptyId, gates)
  publish()
  return () => {
    if (groups.get(ptyId) !== gates) return
    groups.delete(ptyId)
    publish()
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => void listeners.delete(listener)
}

function snapshot(): number {
  return version
}

/**
 * The published group for the first of `ptyIds` that has one, or null.
 *
 * An agent passes its session-slot id plus every tab id, because any one of its
 * panes can be the mounted one; a terminal passes its single id. Same scan, and
 * same order, as `useAttachCapability`, so the attach item and the act behind
 * it can never come from different panes.
 */
export function usePaneInputGroup(ptyIds: string[]): PaneInputGroupGates | null {
  useSyncExternalStore(subscribe, snapshot, snapshot)
  return paneInputGroupFor(ptyIds)
}

/// The same scan without the subscription, for a caller that is not a
/// component. The hook above is what re-renders a menu when a pane mounts,
/// takes over, or goes away.
export function paneInputGroupFor(
  ptyIds: string[],
): PaneInputGroupGates | null {
  for (const id of ptyIds) {
    const gates = groups.get(id)
    if (gates) return gates
  }
  return null
}

/**
 * Does this group have anything of its OWN to render? The attach item is the
 * caller's (see `PaneInputGroupGates`), so the caller ORs it in.
 */
export function paneInputGroupHasItems(
  gates: PaneInputGroupGates | null,
): boolean {
  if (!gates) return false
  return gates.surfaceSwitch || gates.keysToggle
}

/** Test-only: forget every registration between cases. */
export function resetPaneInputGroups(): void {
  groups.clear()
  publish()
}
