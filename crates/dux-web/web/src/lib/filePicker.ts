// THE BROWSER FILE PICKER, the third gesture into the upload journey.
//
// A drag needs a desktop and a paste needs the file to already be on the
// clipboard; a picker needs neither, which is why it is the only entry point a
// phone (no drag gesture at all) or a keyboard-only desktop user has. It feeds
// exactly the same route, destinations, naming and toast ladder as the other
// two: everything here does is hand a `File[]` to the caller.
//
// Everything below is DOM-level and framework-free so it can be unit tested by
// dispatching real events at a real `<input type="file">`; `hooks/use-file-
// picker.tsx` is the two-line React wrapper that owns the hidden input.

// The in-flight open per input element, as the function that abandons it. One
// per element rather than one per module: two panes can each own a picker, and
// one settling the other's promise would be a cross-pane bug. A WeakMap so an
// unmounted pane's input takes its entry with it.
const pending = new WeakMap<HTMLInputElement, () => void>()

/**
 * Open the OS file picker on `input` and resolve with what was chosen.
 *
 * Resolves with an EMPTY array when the user cancels, so the caller has one
 * shape to handle and "nothing picked" is not an error.
 *
 * THREE HARD REQUIREMENTS, each of which has its own test:
 *
 *  - **`value` is cleared before every open.** Picking the SAME file twice in a
 *    row leaves the input's value unchanged, and an unchanged value fires no
 *    `change` event, so the second pick would hang forever.
 *  - **A stale pending open settles when the next one starts.** The `cancel`
 *    event is evergreen-only (Chrome 113 / Safari 16.6 / Firefox 91), so on
 *    anything older a dismissed dialog produces no event whatsoever. Rather
 *    than guess with focus heuristics (which fire on tab switches and on the
 *    dialog itself), the abandoned promise is settled empty the moment the user
 *    opens the picker again. Nothing is ever left awaiting forever.
 *  - **`.click()` is synchronous.** It runs on the caller's stack, inside the
 *    user activation of the click that reached the menu item. `await` anything
 *    first and the activation is spent, and the browser silently refuses to
 *    open the dialog. Nothing in this function may become async.
 */
export function pickFiles(input: HTMLInputElement): Promise<File[]> {
  // Settle whatever the previous open left behind (see the doc comment); this
  // also detaches its listeners, so the two opens cannot both answer.
  pending.get(input)?.()
  return new Promise<File[]>((resolve) => {
    const finish = (files: File[]) => {
      input.removeEventListener("change", onChange)
      input.removeEventListener("cancel", onCancel)
      if (pending.get(input) === abandon) pending.delete(input)
      resolve(files)
    }
    const onChange = () => finish(Array.from(input.files ?? []))
    const onCancel = () => finish([])
    const abandon = () => finish([])
    input.addEventListener("change", onChange)
    input.addEventListener("cancel", onCancel)
    pending.set(input, abandon)
    // Re-picking the same file otherwise fires no `change` at all.
    input.value = ""
    input.click()
  })
}
