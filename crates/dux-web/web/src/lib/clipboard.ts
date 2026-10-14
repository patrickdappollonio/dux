// Copies text to the clipboard, reporting whether it succeeded; callers raise their
// own toast. `navigator.clipboard` exists only in a secure context, and dux is
// routinely served over plain HTTP, so there are two fallbacks.
//
// The first runs `execCommand("copy")` and writes the text from the copy event
// it fires, which moves neither focus nor the user's selection. Focus matters
// here: a terminal whose program asked for focus reports is sent a focus-out and
// a focus-in every time focus leaves it, and an IME composition is cancelled.
//
// The last resort, for a browser that runs no copy event for a gesture with
// nothing selected (Safari), focuses a hidden textarea for the length of the
// copy, and removing it leaves focus on the page body. `keepFocus` hands focus
// back to whatever held it just before; the modern API never moves focus, so a
// copy it completes leaves focus wherever the user has put it since. The
// textarea is never tried mid-composition, because taking focus would cancel
// the user's half-typed text; that copy counts as refused.
export async function copyToClipboard(
  text: string,
  { keepFocus = false }: { keepFocus?: boolean } = {},
): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Permission denied / not focused / insecure context: try the legacy path.
  }
  const viaEvent = copyEventCopy(text)
  if (viaEvent !== null) return viaEvent
  if (composing) return false
  if (!keepFocus) return legacyCopy(text)
  const prior = document.activeElement as
    | (Focusable & { isConnected?: boolean })
    | null
  try {
    return legacyCopy(text)
  } finally {
    if (
      prior &&
      typeof prior.focus === "function" &&
      prior.isConnected !== false &&
      document.activeElement !== (prior as unknown)
    ) {
      prior.focus({ preventScroll: true })
    }
  }
}

/// Focus is the one thing a caller can read and set; structural so a test needs
/// no DOM.
interface Focusable {
  focus(options?: FocusOptions): void
}

/// The copy-event technique: true or false when the browser ran it and said
/// whether the copy landed, null when this browser cannot do it and the textarea
/// is worth trying.
function copyEventCopy(text: string): boolean | null {
  if (
    typeof document === "undefined" ||
    typeof document.addEventListener !== "function" ||
    typeof document.execCommand !== "function"
  ) {
    return null
  }
  let fired = false
  let written = false
  const onCopy = (ev: Event) => {
    fired = true
    const data = (ev as ClipboardEvent).clipboardData
    if (!data) return
    data.setData("text/plain", text)
    ev.preventDefault()
    // Capture on the document runs before xterm's or Monaco's own copy handler,
    // which would otherwise overwrite the data with their selection.
    ev.stopImmediatePropagation()
    written = true
  }
  document.addEventListener("copy", onCopy, true)
  let ok: boolean
  try {
    ok = document.execCommand("copy")
  } catch {
    ok = false
  } finally {
    document.removeEventListener("copy", onCopy, true)
  }
  if (written) return ok
  if (fired) return null
  // No event ran. Without a user activation that is the browser refusing the
  // copy, and the textarea would be refused too, after taking focus for nothing.
  // With one (or no way to tell), this browser simply runs no copy event when
  // nothing is selected.
  const activation =
    typeof navigator === "undefined"
      ? undefined
      : (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation
  return activation && !activation.isActive ? false : null
}

let composing = false

/// Track IME compositions anywhere under `target` (the document, in the app), so
/// a copy never takes focus in the middle of one. Returns the undo.
export function watchComposition(target: EventTarget): () => void {
  const start = () => {
    composing = true
  }
  const end = () => {
    composing = false
  }
  target.addEventListener("compositionstart", start, true)
  target.addEventListener("compositionend", end, true)
  return () => {
    target.removeEventListener("compositionstart", start, true)
    target.removeEventListener("compositionend", end, true)
    composing = false
  }
}

function legacyCopy(text: string): boolean {
  try {
    const ta = document.createElement("textarea")
    ta.value = text
    // Keep it off-screen and non-disruptive while it's briefly in the DOM.
    ta.style.position = "fixed"
    ta.style.top = "0"
    ta.style.left = "0"
    ta.style.opacity = "0"
    document.body.appendChild(ta)
    // `finally` guarantees the textarea is removed even if execCommand throws,
    // so a failed copy can't leak a hidden node into the DOM.
    try {
      ta.focus()
      ta.select()
      return document.execCommand("copy")
    } finally {
      document.body.removeChild(ta)
    }
  } catch {
    return false
  }
}
