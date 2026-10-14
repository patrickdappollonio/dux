// One random id per browser profile, kept in localStorage so every tab of this
// browser presents the same one on its events socket. The server only counts
// with it ("2 browsers"): it is never a credential and decides nothing about
// who may drive, delete or stop what.
//
// Storage can be missing or throw (a private window, blocked site data), so the
// id then lives for this page alone: the page still works and is counted as a
// browser of its own.

export const BROWSER_ID_KEY = "dux:browser-id"

// What the server accepts; anything else found in storage is replaced.
const VALID = /^[A-Za-z0-9_-]{16,64}$/

let pageId: string | null = null

function mint(): string {
  // getRandomValues works on the plain-HTTP origins dux is often served on,
  // where crypto.randomUUID does not exist.
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")
}

export function browserId(): string {
  if (pageId !== null) return pageId
  let id: string | null = null
  try {
    const stored = localStorage.getItem(BROWSER_ID_KEY)
    if (stored !== null && VALID.test(stored)) id = stored
  } catch {
    // Unreadable storage: a fresh id for this page below.
  }
  if (id === null) {
    id = mint()
    try {
      localStorage.setItem(BROWSER_ID_KEY, id)
    } catch {
      // Unwritable storage: the id is this page's alone.
    }
  }
  pageId = id
  return id
}

// Tests only: forget the page's cached id, as a new tab of the same browser would.
export function resetBrowserIdForTests(): void {
  pageId = null
}
