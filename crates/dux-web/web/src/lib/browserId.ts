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

function readStored(): string | null {
  try {
    const stored = localStorage.getItem(BROWSER_ID_KEY)
    if (stored !== null && VALID.test(stored)) return stored
  } catch {
    // Unreadable storage: the caller keeps or mints its own.
  }
  return null
}

export function browserId(): string {
  if (pageId !== null) return pageId
  listenForOtherTabs()
  let id = readStored()
  if (id === null) {
    id = mint()
    try {
      localStorage.setItem(BROWSER_ID_KEY, id)
    } catch {
      // Unwritable storage: the id is this page's alone.
    }
    // Storage is shared without a lock, so another tab minting at the same
    // moment may have written after this one; the stored value is the one
    // every tab converges on.
    id = readStored() ?? id
  }
  pageId = id
  return id
}

let listening = false

// A tab that minted and wrote before another tab's write landed hears about it
// here and adopts what storage now holds. It reads storage rather than the
// event's value, because events for two racing writes can arrive after both
// have landed, and only the last write is the browser's id.
//
// A socket already open keeps presenting the id it opened with until it
// reconnects; the events and terminal sockets present the adopted id from their
// next open. Until then the server counts this tab as a second browser. That is
// accepted rather than forced with a reconnect: the id only feeds a displayed
// count, it decides nothing, and the race needs two tabs minting on the same
// first visit to this browser.
function listenForOtherTabs(): void {
  if (listening || typeof window === "undefined") return
  listening = true
  window.addEventListener("storage", (event) => {
    if (event.key !== BROWSER_ID_KEY || pageId === null) return
    pageId = readStored() ?? pageId
  })
}

// Tests only: forget the page's cached id, as a new tab of the same browser would.
export function resetBrowserIdForTests(): void {
  pageId = null
}
