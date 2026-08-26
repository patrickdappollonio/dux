// THE ONE PLAIN BOUNCE, and the reason it cannot be a bare `pty.connect()`.
//
// An automatic reconnect is a plain attach and never a take-over: no retry, no
// resume, no heal bounce carries the flag, and only a press on the take-over
// button does. The take-over intent enforces that by dying with the socket it
// was armed for, delivered as `onConn("closed")`.
//
// EXCEPT THAT A DELIBERATE REOPEN FIRES NO SUCH CLOSE. `connect()` detaches the
// orphan socket's handlers before closing it, which is precisely what lets a
// take-over's own bounce carry its intent across the reconnect it armed. Every
// OTHER caller of `connect()` inherits that same silence, so an intent that was
// armed and never confirmed on the wire survives their bounce too, and the next
// first resize frame carries the flag:
//
//   with no expected owner, the server grants the transfer unconditionally, so
//   a button labelled Reconnect takes the pty from whoever is typing into it;
//
//   with a STALE expected owner (a self-succession that never landed), the
//   server refuses the transfer, and the pane sits believing it owns a pty at a
//   geometry that was never applied.
//
// So every bounce that is not a take-over goes through here, and here spends the
// intent first. The take-over path deliberately does NOT: it arms and then
// bounces, and that one intent is the only one allowed to ride a reconnect.
import type { PtySocket } from "@/lib/ptySocket"

import type { TakeoverIntent } from "./channels"

/// Reopen this pane's socket as a plain attach. Safe on a null socket (a pane
/// mid-teardown), and safe to call with nothing armed.
export function plainBounce(
  pty: PtySocket | null,
  takeoverIntent: TakeoverIntent,
): void {
  takeoverIntent.clear()
  pty?.connect()
}
