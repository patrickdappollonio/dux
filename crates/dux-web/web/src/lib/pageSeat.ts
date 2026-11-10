// One random id per page load, named by every terminal socket this page opens
// (`?page=`), so the server treats all of this page's terminals as ONE driver
// of a row: an agent and the terminal docked under it never refuse each
// other's keystrokes or resizes, and a take-over through either brings both.
//
// Per page load, not per browser (`browserId.ts`) and not the events id
// (`connection.ts`): a reload, a second tab and a second window are each a
// driver of their own, and the events id changes whenever the events socket
// reconnects. It decides who may drive, so it is never stored, logged or shown.

let seat: string | null = null

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
