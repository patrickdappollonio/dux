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
  push: (bytes: Uint8Array) => Uint8Array
  /// Forget a half-read sequence. The carried bytes belong to a byte stream
  /// that has been replaced, so they are dropped rather than emitted into the
  /// middle of the next one.
  reset: () => void
}

export function createSgrColonNormalizer(): SgrColonNormalizer {
  // The bytes of a possible SGR, starting at ESC. Empty whenever the stream is
  // outside one.
  let carry: number[] = []

  return {
    reset() {
      carry = []
    },
    push(bytes) {
      const out: number[] = []

      const releaseCarry = () => {
        for (const b of carry) out.push(b)
        carry = []
      }

      for (const b of bytes) {
        if (carry.length === 0) {
          if (b === ESC) carry.push(b)
          else out.push(b)
          continue
        }

        if (carry.length === 1) {
          // Just the ESC so far. Only `ESC [` can be an SGR; every other
          // introducer (OSC, DCS, a charset designation, a lone ESC) is none of
          // this normaliser's business and leaves immediately.
          if (b === CSI_BRACKET) {
            carry.push(b)
          } else if (b === ESC) {
            out.push(ESC)
          } else {
            out.push(ESC, b)
            carry = []
          }
          continue
        }

        if (isFinalByte(b)) {
          carry.push(b)
          for (const rewritten of rewriteSequence(carry)) out.push(rewritten)
          carry = []
          continue
        }

        if (isParamByte(b)) {
          carry.push(b)
          if (carry.length > MAX_CARRY) releaseCarry()
          continue
        }

        // Anything else inside a CSI is either an intermediate byte (which
        // makes this not an SGR) or a control character the emulator handles
        // mid-sequence. Either way there is nothing to rewrite, so the carry is
        // let go and this byte is judged on its own.
        releaseCarry()
        if (b === ESC) carry.push(b)
        else out.push(b)
      }

      return Uint8Array.from(out)
    },
  }
}

/// Decide what a complete `ESC [ ... <final>` sequence should be spelled as.
function rewriteSequence(seq: number[]): number[] {
  if (seq[seq.length - 1] !== SGR_FINAL) return seq

  // The parameter bytes, between `ESC [` and the final `m`. A private marker
  // (`<`, `=`, `>`, `?`) means this is not a plain SGR, and `!"#$%&'()*+,-./`
  // cannot appear here at all, since an intermediate byte would have released
  // the carry above.
  const params = seq.slice(2, seq.length - 1)
  for (const b of params) {
    const digit = b >= 0x30 && b <= 0x39
    const separator = b === 0x3a || b === 0x3b
    if (!digit && !separator) return seq
  }

  const text = String.fromCharCode(...params)
  const rewritten = text
    .split(";")
    .map(rewriteParameter)
    .join(";")
  if (rewritten === text) return seq

  const bytes = [ESC, CSI_BRACKET]
  for (let i = 0; i < rewritten.length; i++) bytes.push(rewritten.charCodeAt(i))
  bytes.push(SGR_FINAL)
  return bytes
}

const isDigits = (s: string) => s.length > 0 && /^[0-9]+$/.test(s)

/// Give one semicolon-separated parameter its empty colour-space slot, if it is
/// the three-sub-parameter true-colour form and nothing else.
function rewriteParameter(param: string): string {
  const subs = param.split(":")
  if (subs.length !== 5) return param
  if (subs[0] !== "38" && subs[0] !== "48" && subs[0] !== "58") return param
  if (subs[1] !== "2") return param
  if (!isDigits(subs[2]) || !isDigits(subs[3]) || !isDigits(subs[4]))
    return param
  return `${subs[0]}:2::${subs[2]}:${subs[3]}:${subs[4]}`
}
