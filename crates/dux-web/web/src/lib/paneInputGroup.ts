import { useSyncExternalStore } from "react"

// What one pane's input group would say, keyed by pty id, for whichever top menu
// is on screen above it. Only the pane knows the answers, and every menu that
// must offer them sits outside the pane, so the pane publishes and menus read.
//
// A pane publishes only while it owns the input, so a mounted viewer pane cannot
// shadow a mounted owner pane's answers, and an empty group renders no label and
// no separator.
//
// Keyed rather than a single slot, on the `attachRegistry` precedent: several
// panes can be mounted at once, and a menu must read its own rather than
// whichever mounted last. The key comes from the anchor, the only thing that
// knows what is under it: a companion terminal's pane publishes under the
// terminal's id while the menu over it is its agent's, so a scan derived from
// the menu's subject would find nothing.

/// The input group's items for one pane, as gates rather than as rendering.
/// "Attach a file…" is deliberately not here: `attachRegistry` publishes it
/// under the same pty id on the same condition, and two registries answering
/// one question would eventually disagree.
export interface PaneInputGroupGates {
  /// "Use virtual input", the way back from typing straight into the terminal.
  /// Present only while the virtual input is down, and absent rather than
  /// disabled otherwise, because the bottom `⋯` owns the other direction.
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
 * Publish this pane's input group and return its retirement. Last write wins,
 * and the retirement removes the entry only while it is still the one this call
 * installed, so a replacement pane survives the outgoing pane's late cleanup.
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
 * The published group for the first of `ptyIds` that has one, or null. An agent
 * passes its session-slot id plus every tab id; a terminal passes its one id.
 * The scan and its order match `useAttachCapability`, so the attach item and the
 * act behind it cannot come from different panes.
 */
export function usePaneInputGroup(ptyIds: string[]): PaneInputGroupGates | null {
  useSyncExternalStore(subscribe, snapshot, snapshot)
  return paneInputGroupFor(ptyIds)
}

/// The same scan without the subscription, for a caller that is not a component
/// and so does not need the re-render the hook above provides.
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
 * Does this group have anything of its own to render? The attach item is the
 * caller's, so the caller ORs it in.
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
