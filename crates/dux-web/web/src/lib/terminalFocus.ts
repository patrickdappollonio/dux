// Where keyboard focus belongs when a surface outside the terminal pane finishes
// acting on it.
//
// A header control cannot reach the pane's xterm instance to hand Base UI a
// close-focus target, and that target is not a nicety: a macro pastes text into
// the agent's input without submitting, so the user must be able to press Enter,
// which under Base UI's return-to-trigger default would reopen the menu instead.
//
// The mounted pane registers its typing surface here and retires the
// registration on unmount, the same idiom as `composeInsert.ts` and
// `setActivePtySocket`. No registration means no pane is mounted, and the caller
// correctly falls back to Base UI's default.

export type TerminalFocusTarget = () => HTMLElement | null

let terminalFocusTarget: TerminalFocusTarget | null = null

export function setTerminalFocusTarget(target: TerminalFocusTarget | null): void {
  terminalFocusTarget = target
}

export function peekTerminalFocusTarget(): TerminalFocusTarget | null {
  return terminalFocusTarget
}

// The element typing should return to, or null when no pane is mounted.
export function getTerminalFocusElement(): HTMLElement | null {
  return terminalFocusTarget?.() ?? null
}
