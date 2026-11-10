import { createKeyedRegistry } from "./keyedRegistry"

// What one pane's input group would say, keyed by pty id, for whichever top menu
// is on screen above it. Only the pane knows the answers, and every menu that
// must offer them sits outside the pane, so the pane publishes and menus read.
//
// A pane publishes only while it owns the input, so a mounted viewer pane cannot
// shadow a mounted owner pane's answers, and an empty group renders no heading and
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
  /// The typing-surface switch, in whichever direction applies. Present only
  /// where `compose_bar` is `auto`, which is where a press can take effect.
  surfaceSwitch: boolean
  /// Whether the message box is the live surface, which decides the direction
  /// the switch row offers.
  composeSurface: boolean
  /// The terminal-keys show/hide toggle, wherever a key row is on offer for
  /// this device, shown or hidden. A computer renders it as a row of the group;
  /// a phone renders it as a button in the action cluster instead.
  keysToggle: boolean
}

/// The terminal-keys toggle, named for what a press does. One definition so the
/// menu row and the phone cluster's button cannot disagree about the words.
export function keysToggleLabel(visible: boolean): string {
  return visible ? "Hide terminal keys" : "Show terminal keys"
}

const groups = createKeyedRegistry<PaneInputGroupGates>()

/**
 * Publish this pane's input group and return its retirement. Last write wins,
 * and the retirement removes the entry only while it is still the one this call
 * installed, so a replacement pane survives the outgoing pane's late cleanup.
 */
export function registerPaneInputGroup(
  ptyId: string,
  gates: PaneInputGroupGates,
): () => void {
  return groups.register(ptyId, gates)
}

/**
 * The published group for the first of `ptyIds` that has one, or null. An agent
 * passes its session-slot id plus every tab id; a terminal passes its one id.
 * The scan and its order match `useAttachCapability`, so the attach item and the
 * act behind it cannot come from different panes.
 */
export function usePaneInputGroup(ptyIds: string[]): PaneInputGroupGates | null {
  groups.useVersion()
  return paneInputGroupFor(ptyIds)
}

/// The same scan without the subscription, for a caller that is not a component
/// and so does not need the re-render the hook above provides.
export function paneInputGroupFor(
  ptyIds: string[],
): PaneInputGroupGates | null {
  for (const id of ptyIds) {
    const gates = groups.read(id)
    if (gates) return gates
  }
  return null
}

/** Test-only: forget every registration between cases. */
export function resetPaneInputGroups(): void {
  groups.reset()
}
