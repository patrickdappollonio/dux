// One random id per page load, named by every terminal socket this page opens
// (`?page=`), so the server treats all of this page's terminals as ONE driver
// of a row: an agent and the terminal docked under it never refuse each
// other's keystrokes or resizes, and a take-over through either brings both.
//
// Per page load, not per browser (`browserId.ts`) and not the events id
// (`connection.ts`): a reload, a second tab and a second window are each a
// driver of their own, and the events id changes whenever the events socket
// reconnects. It decides who may drive, so it is never stored, logged or shown.
//
// Every request that can change something names it too (`PAGE_HEADER`, set by
// `apiFetch`), which is how the server knows a change comes from the page that
// drives the row. An editor tab also names the page that opened it
// (`openerPage.ts`), so it may write exactly while that page drives.

export const PAGE_HEADER = "x-dux-page"

/// The shape the server takes as a page id.
const PAGE_ID = /^[A-Za-z0-9_-]{32,64}$/

/// The most ids one request names, the server's own limit.
const MAX_PAGES = 4

let seat: string | null = null
let handed: string[] = []

function mint(): string {
  // getRandomValues works on the plain-HTTP origins dux is often served on,
  // where crypto.randomUUID does not exist. 16 bytes, 32 hex characters: the
  // shortest id the server accepts.
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")
}

export function pageSeatId(): string {
  if (seat === null) seat = mint()
  return seat
}

/// Take the ids the page that opened this tab acts as. True when they differ
/// from the ones already held, so a caller knows a new answer is worth asking for.
export function adoptOpenerPages(ids: readonly unknown[]): boolean {
  const own = pageSeatId()
  const next = ids
    .filter((id): id is string => typeof id === "string" && PAGE_ID.test(id) && id !== own)
    .slice(0, MAX_PAGES - 1)
  const changed = next.join(",") !== handed.join(",")
  handed = next
  return changed
}

/// Whether this tab was handed the page that opened it.
export function actsForOpener(): boolean {
  return handed.length > 0
}

/// Every page this tab acts as: its own first, then its opener's.
export function actingPageIds(): string[] {
  return [pageSeatId(), ...handed]
}
