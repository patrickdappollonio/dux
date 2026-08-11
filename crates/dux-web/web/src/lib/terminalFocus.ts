// Where keyboard focus belongs when a surface OUTSIDE the terminal pane finishes
// acting on it.
//
// The macro quick-picker's desktop entry point lives in the header
// (`InsetHeader`), not in `TerminalPane`, so it cannot reach the pane's xterm
// instance to hand Base UI a close-focus target. That target is not a nicety:
// running a macro pastes text into the agent's input WITHOUT submitting, so the
// user must be able to review it and press Enter — and with Base UI's default
// return-to-trigger, Enter would re-press the popover trigger and reopen the
// menu instead.
//
// This module is the module-scope hand-off between them, the same idiom as
// `composeInsert.ts` and `setActivePtySocket` in `ptySocket.ts`: the mounted
// pane registers its typing surface and retires the registration on unmount, so
// a header control can ask "where does typing go right now?" without reaching
// into React. No registration (no pane mounted) means the caller falls back to
// Base UI's default, which is correct: there is no terminal to focus.

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
