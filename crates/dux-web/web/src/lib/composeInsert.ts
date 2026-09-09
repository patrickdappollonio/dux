// The compose-draft insert sink, which lets a picked macro land in the compose bar's
// draft instead of going straight to the PTY: wherever the compose bar is up it is
// the typing surface, so a macro is a draft the user edits and then sends.
//
// A module-scope handoff because the macro picker's entry point is outside the pane
// while the draft state is inside it. `TerminalPane` registers the sink only while
// the compose bar is actually rendered and retires it the moment that stops being
// true, and with no sink registered the direct-to-PTY path runs.

export type ComposeInsertSink = {
  // Inserts raw macro text into the draft at the caret, appending when no caret state
  // is available. Newlines stay verbatim; the compose Send path transforms them.
  insert: (text: string) => void
  // The compose textarea, so the macro popover can name its close-focus target: focus
  // must land in the draft the macro just joined, not the popover's trigger.
  target: () => HTMLElement | null
}

let composeInsertSink: ComposeInsertSink | null = null

export function setComposeInsertSink(sink: ComposeInsertSink | null): void {
  composeInsertSink = sink
}

export function getComposeInsertSink(): ComposeInsertSink | null {
  return composeInsertSink
}
