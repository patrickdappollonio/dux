// Copy and paste against an xterm instance, and the notifications that report
// how it went.
//
// These two lived at module scope inside `TerminalPane.tsx`, where nothing
// could reach them without mounting xterm. They are here so the thing that
// matters about them is testable on its own: neither raise carries an ID.
//
// A fixed id is a REPLACEMENT instruction, and both of these fire on gestures
// the user repeats freely (copy-on-select fires on every drag). sonner resets a
// toast's remaining time only when its DURATION changes, while re-running its
// close timer on every re-raise, so a repeat on one id restarts the countdown
// and the toast never gets to finish: measured at 90 seconds of "Copied to
// clipboard" across 30 copies. Leaving the id off makes each copy its own event
// on its own clock.
//
// Both take the terminal STRUCTURALLY (the one method each needs) rather than
// as `Terminal`, so a test needs no xterm.

import { copyToClipboard } from "./clipboard"
import { notifyError, notifySuccess } from "./notify"

interface Selectable {
  getSelection(): string
}

interface Pasteable {
  paste(text: string): void
}

/// Copy the terminal's current selection to the browser clipboard.
///
/// `copyToClipboard` writes via the async Clipboard API in a secure context and
/// falls back SYNCHRONOUSLY to an execCommand hidden-textarea over plain-HTTP,
/// so calling this from inside a user gesture (mouseup, keydown, menu click)
/// keeps the write permitted even over a Tailscale plain-HTTP origin.
///
/// `refocus` restores focus once the copy settles: the call sites pass the
/// pane's `focusTypingSurface` so focus lands on the ACTIVE typing surface (the
/// compose textarea when the mobile compose bar is up, xterm otherwise) rather
/// than being hardwired to `term.focus()`.
export async function copyTermSelection(
  term: Selectable,
  refocus: () => void,
): Promise<void> {
  const sel = term.getSelection()
  if (!sel) return
  try {
    const ok = await copyToClipboard(sel)
    if (ok) notifySuccess("Copied to clipboard")
    else notifyError("Couldn't copy to clipboard")
  } finally {
    refocus()
  }
}

/// Paste the BROWSER clipboard into the terminal via the async Clipboard API.
///
/// `readText` needs a secure context (HTTPS/localhost) and THROWS synchronously
/// when `navigator.clipboard` is undefined (plain-HTTP) or `readText` is missing
/// (Firefox web content), so the call must be guarded: a bare `catch` on the
/// promise cannot catch a synchronous throw. The plain-HTTP/Ctrl-v path (handled
/// by xterm's native paste event) stays the secure-context-free fallback.
/// `term.paste` applies bracketed-paste (DECSET 2004) and newline
/// normalization.
///
/// `refocus` mirrors `copyTermSelection`'s.
export async function pasteIntoTerm(
  term: Pasteable,
  refocus: () => void,
): Promise<void> {
  const read = navigator.clipboard?.readText?.()
  if (!read) {
    notifyError("Couldn't read clipboard — use Ctrl+v to paste")
    refocus()
    return
  }
  try {
    term.paste(await read)
  } catch {
    notifyError("Couldn't read clipboard — use Ctrl+v to paste")
  } finally {
    refocus()
  }
}
