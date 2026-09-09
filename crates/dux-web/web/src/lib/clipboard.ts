// Copies text to the clipboard, reporting whether it succeeded; callers raise their
// own toast. `navigator.clipboard` exists only in a secure context, and dux is
// routinely served over plain HTTP, so there is a hidden-textarea fallback.
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Permission denied / not focused / insecure context: try the legacy path.
  }
  return legacyCopy(text)
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
