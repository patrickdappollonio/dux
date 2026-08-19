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
// There were deliberately only three, and the take-over arc added the fourth
// after asking the question this paragraph exists to force: is it really a
// read-only setting wearing a write-back costume? It is not. The take-over
// intent is written by the ownership machine (the button), read and CLEARED by
// the one confirmed socket write, and cleared again by a demotion event and by
// the lifecycle teardown. Three writers and a hot-path reader is exactly the
// shape this file is for. A fifth still has to answer the same question.

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

/// THE TAKE-OVER INTENT. Whether the NEXT resize frame that actually reaches the
/// wire must carry the ownership-transfer flag.
///
/// OWNER: the ownership machine's `takeOver`, which arms it and then bounces the
/// socket. It is state, not a queued closure, and that distinction is the fix:
/// a parked "send this exact frame" closure was lost twice over, once to the
/// gesture coalescer (which keeps the FIRST held direct send and drops later
/// ones) and once to a socket that re-dropped between the decision to send and
/// the send. A flag cannot be coalesced away, because it does not care WHICH
/// frame carries it.
///
/// CONSUMED by the lifecycle's `sendResize` wrapper, and only on a CONFIRMED
/// wire write (`sendResize === true`). A frame the socket silently discarded
/// must leave the intent armed, or the take-over is lost with it.
///
/// ALSO CLEARED, without ever being sent, by a `pty.owner` naming ANOTHER
/// owner (the take-over lost a race; re-arming is the user's to decide) and by
/// the lifecycle teardown (unmount, or a switch to a different target). An
/// owner-cleared `pty.owner` (freed) deliberately does NOT clear it: freed
/// names no winner, so it clears nobody's victory, and the intent must survive
/// the old owner disconnecting mid-bounce or the reap racing a
/// self-succession's in-flight flagged frame.
export type TakeoverIntent = {
  read: () => boolean
  arm: () => void
  clear: () => void
}
