import { createKeyedRegistry } from "./keyedRegistry"

// Where a picked macro goes, keyed by pty id. The Macros controls sit outside the
// pane (the desktop header, the phone flap, the theater pill) and several panes
// can be mounted at once, so each pane publishes its own answer under its own id
// and a control reads the one for the pane it belongs to, never whichever pane
// mounted last.

/// The compose draft, while the compose bar is this pane's typing surface.
export interface ComposeInsertSink {
  /// Inserts raw macro text into the draft at the caret. Newlines stay verbatim;
  /// the compose Send path transforms them.
  insert: (text: string) => void
  /// The compose textarea, so the macro picker can land its close focus in the
  /// draft the macro just joined rather than on its own trigger.
  target: () => HTMLElement | null
}

/// One pane's answer.
export interface PaneMacroTarget {
  /// The full-pane take-over card is up over this pane: somebody else drives
  /// the pty, or nobody does and this pane has not claimed it. The Macros
  /// control is disabled for exactly as long as this holds.
  takeoverCard: boolean
  /// Writes bytes down this pane's own socket, or null while this pane is not
  /// the input owner. Null rather than a gated function, because a write from a
  /// non-owner is either dropped by the server or, against a pty nobody drives,
  /// claims it, and a macro must never be the thing that takes a terminal over.
  send: ((bytes: Uint8Array) => void) | null
  /// The compose draft, or null while the compose bar is not the typing surface.
  compose: ComposeInsertSink | null
}

const targets = createKeyedRegistry<PaneMacroTarget>()

/**
 * Publish this pane's macro target and return its retirement. Last write wins,
 * and the retirement removes the entry only while it is still this call's, so a
 * replacement pane survives the outgoing pane's late cleanup.
 */
export function registerPaneMacroTarget(
  ptyId: string,
  target: PaneMacroTarget,
): () => void {
  return targets.register(ptyId, target)
}

/// The published target for one pty, or null when no pane for it is mounted.
export function paneMacroTargetFor(ptyId: string): PaneMacroTarget | null {
  return targets.read(ptyId) ?? null
}

/// The same lookup, re-rendering the caller whenever any pane republishes.
export function usePaneMacroTarget(ptyId: string): PaneMacroTarget | null {
  targets.useVersion()
  return paneMacroTargetFor(ptyId)
}

/** Test-only: forget every registration between cases. */
export function resetPaneMacroTargets(): void {
  targets.reset()
}
