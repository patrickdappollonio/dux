// The write-back channels: values written from inside the wiring, mid-gesture or
// mid-frame, and read by other machines and by the render.
//
// A read-only setting travels one way and belongs in `liveValues.ts` instead. A
// channel is a one-field box whose type name and doc say WHO writes it, so a
// reader can tell a reported verdict from somebody else's, and it is passed to a
// machine explicitly rather than closed over. Anything added here answers the
// same question first: is it a read-only setting in a write-back costume?

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

/// THE MODIFIER LATCH. The accessory bar's sticky Ctrl/Alt, one-shot.
/// OWNER: the input surface, which writes the visible state and this together.
/// READERS: the key handler and the `onData` transform, which must see a latch
/// armed one keystroke ago, before any re-render could have delivered it.
export type ModifierLatch = Channel<{ ctrl: boolean; alt: boolean }>

/// THE OWNERSHIP VERDICT. Whether THIS client currently drives the PTY.
/// OWNER: the ownership machine, writing synchronously at each transition, ahead
/// of the re-render, because an in-flight keystroke is gated on the new answer.
/// READERS: every write path, and the resize coordinator's owner gate.
export type OwnershipVerdict = Channel<boolean>

/// THE CONNECTION IDENTITY. This pane's PTY-socket connection id, or null, which
/// reads safely as "not us".
/// OWNER: the lifecycle's socket handlers.
/// READERS: the ownership machine's handover comparison, and the upload route,
/// which needs the TERMINAL socket's id rather than the events socket's.
export type ConnectionIdentity = Channel<string | null>

/// THE TAKE-OVER INTENT: whether the next resize frame reaching the wire carries
/// the ownership-transfer flag. Armed by ownership, consumed only after a confirmed
/// resize write, and cleared by a close, a definitive owner frame, or teardown.
/// `expectedOwner` makes self-succession conditional on the named prior owner.
export type TakeoverIntent = {
  read: () => boolean
  expectedOwner: () => string | undefined
  arm: (expectedOwner?: string) => void
  clear: () => void
}
