// THE THREE WRITE-BACK CHANNELS.
//
// A read-only setting travels one way and needs no owner (see `liveValues.ts`).
// These three do not: each is written from inside the wiring, mid-gesture or
// mid-frame, and read both by other machines and by the render. That makes each
// one a place two pieces of code can disagree, so each is named, each says who
// writes it, and each is passed to a machine explicitly rather than being a ref
// the machine happened to close over.
//
// Why a channel rather than a bare ref: a ref says nothing about direction and
// nothing about ownership, and the pane had thirty-four of them. A channel is
// the same one-field box with the ownership written into the type name and the
// doc comment, so a reviewer can tell "this machine reports its verdict here"
// from "this machine reads somebody else's" without tracing every assignment.
//
// There are deliberately only three. If a fourth appears, the question to ask
// first is whether it is really a read-only setting wearing a write-back
// costume.

/// A one-field box. `read` is called on the hot paths (every keystroke, every
/// mouse report), so it stays a property read rather than a subscription.
export type Channel<T> = {
  read: () => T
  write: (value: T) => void
}

/// Build a channel over a plain closure variable.
export function channel<T>(initial: T): Channel<T> {
  let value = initial
  return {
    read: () => value,
    write: (next) => {
      value = next
    },
  }
}

/// Build a channel over an existing React ref, for the two channels whose
/// value the render must also read.
export function refChannel<T>(ref: { current: T }): Channel<T> {
  return {
    read: () => ref.current,
    write: (next) => {
      ref.current = next
    },
  }
}

/// THE MODIFIER LATCH. The accessory bar's sticky Ctrl/Alt, one-shot.
///
/// OWNER: the input surface (`useInputSurface`). It is the only thing that
/// writes a latch, and it writes the visible state and this channel together so
/// they can never diverge.
///
/// READERS: the key handler and the `onData` transform, both of which live
/// inside the lifecycle and must see a latch armed one keystroke ago, before
/// any re-render could have delivered it.
export type ModifierLatch = Channel<{ ctrl: boolean; alt: boolean }>

/// THE OWNERSHIP VERDICT. Whether THIS client currently drives the PTY.
///
/// OWNER: the ownership machine (`useTerminalOwnership`). It writes here at each
/// of its transitions, synchronously, before the re-render that shows the new
/// state lands, because an in-flight keystroke has to be gated by the new answer
/// at once.
///
/// READERS: every write path (`onData`, `onBinary`, the accessory sends, the
/// compose send, the upload sinks) and the resize coordinator's owner gate.
export type OwnershipVerdict = Channel<boolean>

/// THE CONNECTION IDENTITY. This pane's PTY-socket connection id, or null.
///
/// OWNER: the lifecycle's socket handlers (the object itself lives in the
/// ownership hook): `onConnected` learns it from the `connected` frame, and
/// `onOpen`, `onReconnecting`, and the effect cleanup clear it. Null is a
/// real value and reads safely as "not us".
///
/// READERS: the ownership machine (an id comparison is the whole handover
/// decision) and the upload route (the server wants the TERMINAL socket's id,
/// not the events socket's).
export type ConnectionIdentity = Channel<string | null>
