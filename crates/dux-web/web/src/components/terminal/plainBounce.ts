// The one plain bounce: every reconnect that is not a take-over goes through here
// and spends the intent first. A bare `connect()` will not do, because it detaches
// its handlers before closing and so fires no `onConn("closed")` to drop the
// intent, and an intent surviving into the next resize frame either steals the pty
// or strands the pane at a geometry the server refused. Only the take-over path
// arms an intent and bounces, and that one is allowed to ride a reconnect.
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
