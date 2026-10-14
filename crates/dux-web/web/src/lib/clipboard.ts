// Copies text to the clipboard, reporting whether it succeeded; callers raise their
// own toast. `navigator.clipboard` exists only in a secure context, and dux is
// routinely served over plain HTTP, so there is a hidden-textarea fallback.
//
// The fallback focuses its textarea for the length of the copy, and removing it
// leaves focus on the page body. `keepFocus` hands focus back to whatever held
// it just before the fallback ran; the modern API never moves focus, so a copy
// it completes leaves focus wherever the user has put it since.
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
