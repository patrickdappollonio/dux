// Has the app's own connection stopped trying? One boolean, published by the
// store as the events socket's plan changes and read by every PTY socket as part
// of its retry gate.
//
// A terminal follows the app socket. Once the events socket has spent its budget
// the page is in front of a server it cannot reach and says so; a terminal still
// opening sockets underneath that would be spending the same dead network with
// nothing on screen to show for it. PTY sockets keep their own state and their
// own backoff: this holds the next attempt rather than ending the loop, so the
// moment the app socket is back they resume unprompted, exactly as the
// run-identity gate beside them behaves.
//
// It is its own module for the same reason `serverValidated` is: the PTY socket
// must read it without importing the store, which imports the PTY socket.

let givenUp = false

/// Publish the events socket's give-up state. Called from the store's `onPlan`
/// handler, which is the only writer.
export function setAppSocketGivenUp(value: boolean): void {
  givenUp = value
}

/// Whether the app socket has stopped trying.
export function appSocketGivenUp(): boolean {
  return givenUp
}
