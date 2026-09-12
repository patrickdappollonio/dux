/**
 * Spelling the colon form of a true-colour SGR the one way xterm.js reads
 * correctly.
 *
 * ITU T.416 gives `38:2` an optional colour-space slot before the channels, so
 * both `38:2::R:G:B` and `38:2:R:G:B` are in the wild; Neovim, tmux and a good
 * many others emit the shorter one. Alacritty, kitty, iTerm2, Terminal.app and
 * dux's own emulator in dux-core all read the shorter one as RGB. xterm.js does
 * not: with exactly three sub-parameters after the `2` it takes the first as a
 * colour-space id, so the channels shift left, the blue channel is lost, teal
 * (0,128,128) paints as olive (128,128,0) and white paints as yellow.
 *
 * So the bytes are respelled on their way into xterm: the shorter form gains
 * the empty slot, and nothing else in the stream is touched. This can be
 * deleted whole if xterm.js ever accepts the three-sub-parameter form itself.
 *
 * The normaliser is a stream: a sequence may be cut in half by a websocket
 * frame boundary at any byte, so a possible SGR is carried between calls and
 * everything else is emitted at once. It works on bytes rather than text
 * because every byte of a CSI is ASCII, so a multi-byte UTF-8 character (whose
 * bytes are all >= 0x80) can never be mistaken for part of one.
 */

const ESC = 0x1b
const CSI_BRACKET = 0x5b // [
const SGR_FINAL = 0x6d // m
const COLON = 0x3a
const SEMICOLON = 0x3b

/// A CSI parameter byte: digits, `:`, `;`, and the private markers `<=>?`.
const isParamByte = (b: number) => b >= 0x30 && b <= 0x3f
/// A CSI final byte, which ends the sequence and says what it was.
const isFinalByte = (b: number) => b >= 0x40 && b <= 0x7e

/// How much of a possible CSI is worth carrying before giving up on it. A real
/// SGR is a few dozen bytes; anything longer is a stream that will never
/// terminate, and holding it back would stall the picture. Past this the carry
/// is released verbatim and the rest of that sequence flows straight through.
const MAX_CARRY = 512

export type SgrColonNormalizer = {
  /// Respell this chunk and return what may be emitted now. Bytes that could
  /// still turn out to be the head of an SGR are carried to the next call.
  ///
  /// The result MAY BE THE CALLER'S OWN ARRAY, returned by identity when the
  /// chunk holds no escape and nothing is carried, which is most of what a pty
  /// sends. Callers must therefore neither mutate it nor assume it is a private
  /// copy. Everything else comes back in a buffer of the normaliser's own,
  /// which it does not retain.
  push: (bytes: Uint8Array) => Uint8Array
  /// Forget a half-read sequence. The carried bytes belong to a byte stream
  /// that has been replaced, so they are dropped rather than emitted into the
  /// middle of the next one.
  reset: () => void
}

export function createSgrColonNormalizer(): SgrColonNormalizer {
  // The bytes of a possible SGR, starting at ESC, in a buffer sized for the
  // most this will ever hold. `carryLen` is zero whenever the stream is outside
  // a sequence, which is the state the fast path below tests for.
  const carry = new Uint8Array(MAX_CARRY + 2)
  let carryLen = 0

  return {
    reset() {
      carryLen = 0
    },
    push(bytes) {
      // THE FAST PATH, and the one that matters: a chunk with no escape in it
      // while nothing is carried is already its own answer. Scanning for the
      // escape is a fraction of the cost of walking the bytes, and returning
      // the caller's array spares the copy as well. See `push` on the type for
      // what that aliasing asks of callers.
      if (carryLen === 0 && bytes.indexOf(ESC) === -1) return bytes

      // Room for the chunk, whatever was carried in, and the handful of bytes a
      // respelling adds. `ensure` grows it if a chunk turns out to be dense
      // enough in rewrites to need more.
      let out = new Uint8Array(bytes.length + carryLen + 64)
      let n = 0

      const ensure = (extra: number) => {
        if (n + extra <= out.length) return
        let cap = out.length * 2
        while (cap < n + extra) cap *= 2
        const bigger = new Uint8Array(cap)
        bigger.set(out.subarray(0, n), 0)
        out = bigger
      }
      const emit = (b: number) => {
        ensure(1)
        out[n++] = b
      }
      const releaseCarry = () => {
        ensure(carryLen)
        out.set(carry.subarray(0, carryLen), n)
        n += carryLen
        carryLen = 0
      }

      let i = 0
      while (i < bytes.length) {
        if (carryLen === 0) {
          // Outside a sequence, so everything up to the next escape is passed
          // through in one copy rather than one byte at a time.
          const next = bytes.indexOf(ESC, i)
          const end = next === -1 ? bytes.length : next
          if (end > i) {
            ensure(end - i)
            out.set(bytes.subarray(i, end), n)
            n += end - i
            i = end
            continue
          }
          carry[carryLen++] = ESC
          i++
          continue
        }

        const b = bytes[i++]

        if (carryLen === 1) {
          // Just the ESC so far. Only `ESC [` can be an SGR; every other
          // introducer (OSC, DCS, a charset designation, a lone ESC) is none of
          // this normaliser's business and leaves immediately.
          if (b === CSI_BRACKET) {
            carry[carryLen++] = b
          } else if (b === ESC) {
            emit(ESC)
          } else {
            emit(ESC)
            emit(b)
            carryLen = 0
          }
          continue
        }

        if (isFinalByte(b)) {
          carry[carryLen++] = b
          const rewritten = rewriteSequence(carry, carryLen)
          if (rewritten === null) {
            releaseCarry()
          } else {
            ensure(rewritten.length)
            out.set(rewritten, n)
            n += rewritten.length
            carryLen = 0
          }
          continue
        }

        if (isParamByte(b)) {
          carry[carryLen++] = b
          if (carryLen > MAX_CARRY) releaseCarry()
          continue
        }

        // Anything else inside a CSI is either an intermediate byte (which
        // makes this not an SGR) or a control character the emulator handles
        // mid-sequence. Either way there is nothing to rewrite, so the carry is
        // let go and this byte is judged on its own.
        releaseCarry()
        if (b === ESC) carry[carryLen++] = ESC
        else emit(b)
      }

      return out.subarray(0, n)
    },
  }
}

/// Decide what a complete `ESC [ ... <final>` sequence should be spelled as.
/// Returns null when it should be emitted exactly as it arrived, which is the
/// overwhelmingly common answer.
function rewriteSequence(seq: Uint8Array, len: number): Uint8Array | null {
  if (seq[len - 1] !== SGR_FINAL) return null

  // The parameter bytes, between `ESC [` and the final `m`. A private marker
  // (`<`, `=`, `>`, `?`) means this is not a plain SGR, and `!"#$%&'()*+,-./`
  // cannot appear here at all, since an intermediate byte would have released
  // the carry above.
  //
  // A rewrite needs a colon, so a parameter list without one is answered here
  // and never reaches the string work: that is every ordinary SGR, the
  // semicolon true-colour form included.
  let sawColon = false
  for (let i = 2; i < len - 1; i++) {
    const b = seq[i]
    if (b >= 0x30 && b <= 0x39) continue
    if (b === 0x3b) continue
    if (b === 0x3a) {
      sawColon = true
      continue
    }
    return null
  }
  if (!sawColon) return null

  // Count the parameters that need the slot before building anything. Almost
  // every sequence that gets this far needs none, and answering it without
  // allocating is what keeps heavily-coloured output cheap.
  const from = 2
  const to = len - 1
  let needed = 0
  let start = from
  for (let i = from; i <= to; i++) {
    if (i === to || seq[i] === SEMICOLON) {
      if (needsColourSpaceSlot(seq, start, i)) needed++
      start = i + 1
    }
  }
  if (needed === 0) return null

  const bytes = new Uint8Array(len + needed)
  bytes[0] = ESC
  bytes[1] = CSI_BRACKET
  let n = 2
  start = from
  for (let i = from; i <= to; i++) {
    if (i !== to && seq[i] !== SEMICOLON) continue
    if (needsColourSpaceSlot(seq, start, i)) {
      // `38:2:` then the empty colour-space slot, then the channels: the same
      // bytes with one extra colon after the second sub-parameter.
      const afterKind = start + 4
      bytes.set(seq.subarray(start, afterKind), n)
      n += 4
      bytes[n++] = COLON
      bytes.set(seq.subarray(afterKind, i), n)
      n += i - afterKind
    } else {
      bytes.set(seq.subarray(start, i), n)
      n += i - start
    }
    if (i !== to) bytes[n++] = SEMICOLON
    start = i + 1
  }
  bytes[n] = SGR_FINAL
  return bytes
}

/// Whether one semicolon-separated parameter, spanning `[start, end)`, is the
/// three-sub-parameter true-colour form that needs its empty colour-space slot.
///
/// Every byte in the span is already known to be a digit or a colon, so a
/// sub-parameter is all digits exactly when it is not empty.
function needsColourSpaceSlot(
  seq: Uint8Array,
  start: number,
  end: number,
): boolean {
  // `38:2:R:G:B` at its very shortest is `38:2:0:0:0`, ten bytes.
  if (end - start < 10) return false
  // The kind: `38`, `48` or `58`, then a colon, then `2`, then a colon.
  const kind = seq[start]
  if (kind !== 0x33 && kind !== 0x34 && kind !== 0x35) return false
  if (seq[start + 1] !== 0x38) return false
  if (seq[start + 2] !== COLON) return false
  if (seq[start + 3] !== 0x32) return false
  if (seq[start + 4] !== COLON) return false

  // Exactly two more colons, neither of them leading, trailing or adjacent, so
  // all three channels are present and none is empty.
  let colons = 0
  let previous = start + 4
  for (let i = start + 5; i < end; i++) {
    if (seq[i] !== COLON) continue
    if (i === previous + 1) return false
    colons++
    if (colons > 2) return false
    previous = i
  }
  if (colons !== 2) return false
  return previous !== end - 1
}
