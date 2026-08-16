import type * as React from "react"
import { useCallback, useRef } from "react"

import { pickFiles } from "@/lib/filePicker"

// The React wrapper around `lib/filePicker`: it owns the hidden
// `<input type="file" multiple>` the browser needs in the document, and hands
// back the one function that opens it.
//
// The element comes back as `input` (rather than a ref the caller wires up) so
// a surface cannot half-adopt the picker: rendering `{input}` somewhere in its
// tree is the whole integration. `open()` must be called SYNCHRONOUSLY from the
// activating click handler, or the user activation is spent and no dialog
// appears; see `pickFiles`.
export function useFilePicker(): {
  input: React.ReactElement
  open: () => Promise<File[]>
} {
  const ref = useRef<HTMLInputElement | null>(null)
  const open = useCallback(() => {
    const el = ref.current
    // An unmounted picker picks nothing, which is the same answer as a cancel:
    // callers have exactly one empty case to handle.
    return el ? pickFiles(el) : Promise.resolve<File[]>([])
  }, [])
  const input = (
    <input
      ref={ref}
      type="file"
      multiple
      // `hidden` rather than a visually-hidden class: nothing focuses it, it is
      // never in the tab order, and it exists only to be clicked in code.
      hidden
      // It is not a form control anybody fills in, so keep it out of the
      // accessibility tree entirely; the menu item is the named control.
      aria-hidden="true"
      tabIndex={-1}
      data-testid="file-picker-input"
    />
  )
  return { input, open }
}
