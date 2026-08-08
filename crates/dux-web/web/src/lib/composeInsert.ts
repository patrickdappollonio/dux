// The compose-draft insert sink: the bridge that lets a picked macro land in
// the mobile compose bar's DRAFT instead of going straight to the PTY.
//
// On a phone the compose bar is the typing surface (see the CLAUDE.md web
// tenet), so a macro is a draft the user edits and then Sends — not an
// immediate wire write. But the macro quick-picker's mobile entry point lives
// in the terminal screen's HEADER (MobileShell), outside `TerminalPane`, while
// the draft state lives inside the pane. This module is the module-scope
// hand-off between them, the same idiom as `setActivePtySocket` in
// `ptySocket.ts`: `TerminalPane` registers the sink for exactly the window in
// which the compose bar is actually rendered (mobile, `ui.compose_bar` on,
// input owner) and retires it the moment any of that stops being true, so the
// store's `runMacro` can ask "is a compose draft the destination right now?"
// without reaching into React. No sink registered means the direct-to-PTY
// path runs exactly as before (desktop, preference off, non-owner viewer).

export type ComposeInsertSink = {
  // Insert RAW macro text into the compose draft at the caret (appending when
  // the caret state is unavailable), preserving the rest of the draft. The
  // text keeps its newlines verbatim; the compose Send path owns the
  // newline→keystroke wire transform later.
  insert: (text: string) => void
  // The compose textarea, so the macro popover can hand Base UI the right
  // close-focus target (focus must land in the draft the macro just joined,
  // not back on the popover's trigger button).
  target: () => HTMLElement | null
}

let composeInsertSink: ComposeInsertSink | null = null

export function setComposeInsertSink(sink: ComposeInsertSink | null): void {
  composeInsertSink = sink
}

export function getComposeInsertSink(): ComposeInsertSink | null {
  return composeInsertSink
}
