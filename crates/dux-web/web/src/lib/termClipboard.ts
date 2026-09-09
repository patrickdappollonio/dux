// Copy and paste against an xterm instance, and the notifications that report
// how it went.
//
// No raise here carries a toast id. An id is a REPLACEMENT instruction, and
// these fire on gestures the user repeats freely (copy-on-select fires on every
// drag); sonner re-runs a toast's close timer on every re-raise of one id, so
// the countdown restarts and the toast never gets to finish. Without an id each
// copy is its own event on its own clock.
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
/// Must be called from inside a user gesture: over a plain-HTTP origin
/// `copyToClipboard` falls back SYNCHRONOUSLY to a hidden-textarea execCommand,
/// which only a gesture permits. `refocus` restores focus once the copy settles,
/// and callers pass the pane's `focusTypingSurface` so it lands on the ACTIVE
/// typing surface rather than always on xterm.
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
/// `readText` needs a secure context and THROWS synchronously when
/// `navigator.clipboard` is undefined (plain-HTTP) or `readText` is missing
/// (Firefox web content), so the call must be guarded: a `catch` on the promise
/// cannot catch a synchronous throw. Ctrl-v, handled by xterm's native paste
/// event, is the secure-context-free fallback. `term.paste` applies bracketed
/// paste and newline normalization; `refocus` mirrors `copyTermSelection`'s.
export async function pasteIntoTerm(
  term: Pasteable,
  refocus: () => void,
): Promise<void> {
  await pasteClipboardText((text) => term.paste(text), refocus)
}

/// Read the BROWSER clipboard and hand the text to whatever the typing surface
/// is right now.
///
/// The terminal is not always the destination: while the message box is up it IS
/// the typing surface, and a right-click that dropped the clipboard into the PTY
/// behind an unsent draft would type past the buffer, the very thing the box
/// exists to stop. `pasteIntoTerm` is one call to this, with the same guards and
/// the same refusal message.
export async function pasteClipboardText(
  deliver: (text: string) => void,
  refocus: () => void,
): Promise<void> {
  const read = navigator.clipboard?.readText?.()
  if (!read) {
    notifyError("Couldn't read clipboard, use Ctrl+v to paste")
    refocus()
    return
  }
  try {
    deliver(await read)
  } catch {
    notifyError("Couldn't read clipboard, use Ctrl+v to paste")
  } finally {
    refocus()
  }
}
