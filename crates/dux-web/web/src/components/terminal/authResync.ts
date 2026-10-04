// A terminal that stayed open across a sign-out re-asserts its size when the
// page signs in again. Every resize sent while signed out was refused
// (`ptySocket.ts` sends nothing then), and the window may have changed under
// the login page. The re-assert goes through the resize coordinator's own
// foreground resync, never around it: a watcher still sends nothing, and a pty
// nobody owns is not claimed by a page that merely came back. A socket that
// closed instead asserts its size on its own reopen.

import { onAuthOpen } from "@/lib/authGate"

export function resyncOnSignIn(deps: {
  isOpen: () => boolean
  resync: () => void
}): () => void {
  return onAuthOpen(() => {
    if (deps.isOpen()) deps.resync()
  })
}
