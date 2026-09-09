// The browser file picker, the third gesture into the upload journey and the
// only one a phone or a keyboard-only user has. It hands a `File[]` to the
// caller, which feeds the same route, naming and toast ladder as a drop or a
// paste. `hooks/use-file-picker.tsx` is the React wrapper owning the input.

// The in-flight open per input element, as the function that abandons it. One
// per element rather than one per module, because two panes can each own a
// picker; a WeakMap, so an unmounted pane's input takes its entry with it.
const pending = new WeakMap<HTMLInputElement, () => void>()

/**
 * Open the OS file picker on `input` and resolve with what was chosen, or with
 * an empty array when the user cancels, so a cancel is not an error.
 *
 * Three requirements, each with its own test:
 *
 *  - `value` is cleared before every open: re-picking the same file leaves the
 *    value unchanged, which fires no `change`, and the pick would hang.
 *  - A stale pending open settles when the next one starts: the `cancel` event
 *    is evergreen-only, so an older browser reports a dismissal not at all.
 *  - `.click()` is synchronous, inside the user activation of the click that
 *    reached the menu item. Nothing in this function may become async, or the
 *    activation is spent and the browser refuses to open the dialog.
 */
export function pickFiles(input: HTMLInputElement): Promise<File[]> {
  // Settle whatever the previous open left behind; this also detaches its
  // listeners, so the two opens cannot both answer.
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
