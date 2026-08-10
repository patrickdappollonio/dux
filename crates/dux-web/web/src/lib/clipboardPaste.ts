// Pasting out of the clipboard onto an agent or a terminal: the pure decision,
// and the names an unnamed clipboard image and a filed-away long text paste are
// saved under.
//
// TWO TRIGGERS, ONE JOURNEY. The first is an image: take a screenshot, press
// paste, hand the picture to the agent. The second is a very LONG piece of
// text. Both end the same way, because dropping a file already did all of this
// (`fileDrop.ts`): the file is saved through the upload route and its PATH is
// pasted. Nothing here uploads, writes, or touches a terminal; the component
// applies the decision and reuses the drop path.
//
// WHY A LONG TEXT PASTE BECOMES A FILE. An agent has a limited context window,
// but it can read or scan a document efficiently when it needs to. A wall of
// pasted text spends that context whether or not the agent needs all of it; a
// path costs almost nothing and the agent fetches what it wants.
//
// WHY THE `paste` EVENT AND NOT `navigator.clipboard.read()`. dux is routinely
// served over plain HTTP on a Tailscale address, and the async Clipboard API's
// read is blocked outside a secure context. That is not a corner case, it is
// the deployment the feature has to work in, and it is the same constraint
// already written down for right-click paste in the CLAUDE.md clipboard tenet.
// The `paste` event's `clipboardData` carries the bytes with no secure-context
// requirement at all, because the user gesture IS the permission.
//
// Free of any React/xterm/DOM-lookup import, like `termkeys.ts` and
// `composebar.ts`, so the whole matrix is unit-testable without mounting a
// terminal (see `clipboardPaste.test.ts`).

/// A `DataTransferItem`, structurally. The real type cannot be constructed in
/// a test and carries a great deal this decision never reads, so the three
/// members that matter are named here instead.
export type ClipboardItemLike = {
  readonly kind: string
  readonly type: string
  getAsFile: () => File | null
}

/// What is being pasted INTO, and it is a union rather than a flag so the
/// agents-only rule is STRUCTURAL: a terminal carries no threshold at all, so
/// there is nothing a later edit could accidentally invert to start filing a
/// terminal's pastes away.
///
/// A long paste into a shell is usually a command or a heredoc, and turning
/// that into a file would destroy what the user meant. A terminal keeps pasting
/// text verbatim at any length, and always will.
export type ClipboardPastePane =
  /// An agent pane. `longTextChars` is `ui.upload_pasted_text_chars`: a text
  /// paste longer than this many CHARACTERS is saved as a file instead of being
  /// typed. `0` switches the behaviour off, and so does any value an older
  /// server never sent (the browser reads an absent field as `0`, the same
  /// "not yet known is not enabled" rule `file_drop_max_bytes` follows).
  | { kind: "agent"; longTextChars: number }
  /// A terminal pane. No threshold, on purpose. See above.
  | { kind: "terminal" }

/// The runtime facts the decision folds in beside the clipboard's own contents.
export type ClipboardPasteContext = {
  /// Whether the upload feature exists at all on this server
  /// (`[server] file_drop_max_bytes` above zero). When it does not, an image
  /// paste behaves exactly as it did before this feature was written.
  uploadsEnabled: boolean
  /// Whether this client holds input for the pane being pasted into.
  isOwner: boolean
  /// Whether the user asked for a TEXT paste specifically, with the
  /// `Ctrl+Shift+v` / `Cmd+Shift+v` chord (`termkeys::forcesTextPaste`). When
  /// set, image handling is skipped entirely and the paste is left to xterm,
  /// which is the escape hatch out of image-wins: rich content (a copied
  /// spreadsheet range, say) carries an `image/png` flavour beside its text,
  /// and without this the numbers would be unreachable.
  ///
  /// It is ONE hatch for both triggers rather than two competing ones: the same
  /// chord means "give it to me literally", so it beats the long-text rule too
  /// and a user who wants their 5000 characters typed at the prompt has exactly
  /// one thing to press.
  forceText: boolean
  /// Which kind of pane this paste landed on. See [`ClipboardPastePane`].
  pane: ClipboardPastePane
}

/// What the pane must do with one `paste` event.
///
/// The three outcomes the feature was specified around are `ignore` (there is
/// nothing here we could act on), `upload` (an image, take it over) and
/// `xterm` (an ordinary paste, leave it exactly as it was). `refused` is a
/// FOURTH, added deliberately: an image paste from a client that does not hold
/// input must save nothing AND say so, and folding that into `ignore` would
/// have left the user with a keystroke that silently did nothing.
///
/// `ignore` and `xterm` produce the same inaction in today's callers, and they
/// are still kept apart, because they are different statements about what was
/// on the clipboard: one says "no text either", the other says "there is text
/// and it belongs to somebody else". A caller that has to tell them apart (a
/// surface with no text handler of its own) then can.
export type ClipboardPasteAction =
  | { kind: "ignore" }
  | { kind: "xterm" }
  | {
      kind: "upload"
      files: File[]
      /// Set ONLY when these files are not files at all but one long TEXT paste
      /// dux turned into a document, and then it is that paste's character
      /// count. It travels to the toast, which has to say what happened: a
      /// paste that silently becomes a path is a surprise, so the report leads
      /// with the size that triggered it. Absent for an image paste.
      pastedTextChars?: number
    }
  | {
      kind: "refused"
      /// What was turned away. It exists so the caller can give the two
      /// refusals two toast ids: on one shared id an image refusal and a text
      /// refusal replace each other, so a user who pastes both while watching
      /// only ever sees the second and is told nothing about the first.
      subject: "image" | "text"
      reason: string
    }

/// Why an image paste is refused for a client that is only watching, as the
/// whole sentence the toast shows. It says the outcome first (nothing was
/// saved, so there is no stray file to hunt for) and then the way out, because
/// taking over is one tap away.
export const NOT_OWNER_IMAGE_PASTE_REASON =
  "The image was not saved: another device is driving this session. Take over to paste it here."

/// The same, for a long text paste that would have become a file. Its own
/// sentence rather than a shared one, because the subject differs and "the
/// image was not saved" would be a lie about a paragraph of text.
export const NOT_OWNER_TEXT_PASTE_REASON =
  "The pasted text was not saved: another device is driving this session. Take over to paste it here."

/// The extension for an image the clipboard handed over with no name, keyed by
/// mime type.
///
/// Small and explicit rather than derived from the subtype, because the two
/// disagree exactly where it matters: `image/jpeg` is conventionally `.jpg`
/// and `image/svg+xml` would otherwise become `.svg+xml`.
const IMAGE_EXTENSIONS: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/avif": "avif",
  "image/bmp": "bmp",
  "image/tiff": "tiff",
  "image/svg+xml": "svg",
}

/// The extension used when the mime type is unknown or missing. Deliberately
/// not derived from the type string: a subtype can carry characters that have
/// no business in a file name, and the server VALIDATES names rather than
/// rewriting them, so a name dux invents must be one it would accept.
const FALLBACK_IMAGE_EXTENSION = "img"

/// The extension a filed-away text paste gets. Plain text, so the user, the
/// agent and any editor all read the same thing.
const TEXT_PASTE_EXTENSION = "txt"

/// The mime type the synthesised text file carries. Spelled with the charset
/// because the bytes really are UTF-8: the `Blob` constructor encodes a
/// JavaScript string that way, which is what makes the file a byte-for-byte
/// copy of what was on the clipboard, emoji and non-Latin text included.
const TEXT_PASTE_MIME = "text/plain;charset=utf-8"

function two(n: number): string {
  return String(n).padStart(2, "0")
}

/// The `pasted-<local clock>` stem both synthesised names share.
///
/// Local time, not UTC: the user reading the folder is on the same clock they
/// pressed paste by. The moment is passed in rather than read, so the names are
/// pinned by tests rather than by whenever the suite happens to run.
function pastedStem(now: Date): string {
  return (
    `pasted-${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}` +
    `-${two(now.getHours())}${two(now.getMinutes())}${two(now.getSeconds())}`
  )
}

/// What to call a pasted image.
///
/// The clipboard's own name WINS whenever there is one, for the same reason
/// the upload route never rewrites a dropped name: it is the user's, and
/// accented and non-Latin names have to survive. Chrome and Firefox both hand
/// a pasted screenshot over as `image.png`, so that is the ordinary case and
/// several pastes collide; that is fine and is not worked around here, because
/// the server's collision suffix already guarantees nothing is overwritten and
/// the toast reports the name the file ended up with.
///
/// Only a MISSING name is invented, and it is built from the clock so two
/// pastes in one session are distinguishable in the folder listing without
/// having to open them. The moment is passed in rather than read, so the whole
/// thing is pure and the names are pinned by tests rather than by whenever the
/// suite happens to run. Local time, not UTC: the user reading the folder is
/// on the same clock they took the screenshot by.
export function pastedImageName(
  name: string,
  type: string,
  now: Date,
): string {
  const given = name.trim()
  if (given !== "") return name
  const ext = IMAGE_EXTENSIONS[type.toLowerCase()] ?? FALLBACK_IMAGE_EXTENSION
  return `${pastedStem(now)}.${ext}`
}

/// What to call the file a long text paste becomes.
///
/// The same shape as [`pastedImageName`]'s invented name, deliberately: these
/// are the two things dux itself names, they land in the same folder, and a
/// reader scanning that folder should see one convention rather than two. There
/// is no clipboard-supplied name to prefer here, because text has none, so this
/// branch is all there is.
export function pastedTextName(now: Date): string {
  return `${pastedStem(now)}.${TEXT_PASTE_EXTENSION}`
}

/// Whether one clipboard item is an image FILE. Both halves are required: an
/// `image/svg+xml` item of kind `string` is markup somebody copied out of an
/// editor, and pasting that as an attachment rather than as text would be
/// wrong.
function isImageFile(item: ClipboardItemLike): boolean {
  return item.kind === "file" && item.type.toLowerCase().startsWith("image/")
}

/// What the pane should do with the contents of one `paste` event.
///
/// An image WINS over text in the same event, which is the case that actually
/// arrives: copying a screenshot out of an application routinely puts an
/// `image/png` on the clipboard beside a `text/html` snapshot of it, and
/// letting both through would paste the path and then dump markup after it.
///
/// Anything that is not an image paste is left completely alone, so ordinary
/// text paste behaves exactly as it does today: it is xterm's own `paste`
/// handler (fed by the browser's native event) that reads it, applies
/// bracketed paste and writes it out, and none of that is re-implemented here.
///
/// Non-image files are deliberately not accepted. See the test of the same
/// name: a `kind: "file"` item that is not an image is usually an artifact of
/// how an application puts rich content on the clipboard, and treating one as
/// an attachment would hijack a paste the user meant as text.
///
/// That same argument applies to an `image/png` from a rich source, which is
/// why `ctx.forceText` exists: `Ctrl+Shift+v` skips image handling outright and
/// takes the text, so image-wins is a default rather than a trap.
///
/// `text` is the clipboard's `text/plain` flavour, read SYNCHRONOUSLY by the
/// caller (`clipboardData.getData("text/plain")`). It is a parameter rather
/// than something read from `items` because a `DataTransferItem` of kind
/// `string` only yields its contents through an async callback, and this
/// decision has to be made before the `paste` event finishes being dispatched
/// or there is no cancelling it. Empty string when the clipboard carries none.
export function clipboardPasteAction(
  items: readonly ClipboardItemLike[],
  text: string,
  ctx: ClipboardPasteContext,
  now: Date,
): ClipboardPasteAction {
  const hasText = items.some((i) => i.kind === "string")
  const fallback: ClipboardPasteAction = hasText
    ? { kind: "xterm" }
    : { kind: "ignore" }
  if (!ctx.uploadsEnabled) return fallback
  // Before the ownership gate as well as before the upload: a forced text paste
  // has refused nothing, so a viewer using the hatch must not be told an image
  // was turned away.
  if (ctx.forceText) return fallback

  // Resolved BEFORE the ownership gate, so a clipboard carrying no usable
  // image is never reported as a refusal: a viewer pasting plain text has had
  // nothing refused and must not be told otherwise.
  const files: File[] = []
  for (const item of items) {
    if (!isImageFile(item)) continue
    const file = item.getAsFile()
    if (file === null) continue
    files.push(
      new File([file], pastedImageName(file.name, file.type || item.type, now), {
        type: file.type || item.type,
      }),
    )
  }
  if (files.length > 0) {
    if (!ctx.isOwner) {
      return {
        kind: "refused",
        subject: "image",
        reason: NOT_OWNER_IMAGE_PASTE_REASON,
      }
    }
    return { kind: "upload", files }
  }

  // No image. A long enough TEXT paste onto an AGENT becomes a document, for
  // the context-window reason at the top of this file. Checked after the image
  // so a paste carrying both still prefers the image, which is the thing the
  // user was looking at when they copied.
  return longTextPasteAction(text, ctx, now) ?? fallback
}

/// The long-text half of the decision, or `null` when this paste is not one.
///
/// Split out so the two triggers read as two rules rather than one long
/// staircase, and so the terminal exclusion sits at the top of the rule it
/// belongs to.
function longTextPasteAction(
  text: string,
  ctx: ClipboardPasteContext,
  now: Date,
): ClipboardPasteAction | null {
  // A TERMINAL has no threshold to read, by construction. This is not a
  // condition guarding a value that exists; there is nothing there.
  if (ctx.pane.kind !== "agent") return null
  const limit = ctx.pane.longTextChars
  // `0` is the documented "switch this off" value, and it is also what the
  // browser sees from a server too old to publish the setting.
  if (limit <= 0) return null
  // Ask only the question that decides the branch: is it OVER the limit. The
  // count stops at `limit + 1`, so an ordinary paste costs O(limit) no matter
  // how big the clipboard is, and this runs inside the `paste` handler before
  // anything has been cancelled.
  //
  // Strictly greater, so a paste of exactly the threshold is still typed.
  if (countCodePoints(text, limit + 1) <= limit) return null
  if (!ctx.isOwner) {
    return {
      kind: "refused",
      subject: "text",
      reason: NOT_OWNER_TEXT_PASTE_REASON,
    }
  }
  // Only now is the exact size worth a full pass: the toast reports it, and it
  // is the number the user acts on when deciding where to put the threshold.
  // This paste is already becoming a file and going over the network, so one
  // allocation-free scan beside that is nothing.
  const chars = countCodePoints(text)
  // The `Blob` constructor encodes the string as UTF-8, so the file is the
  // pasted string byte for byte: no BOM added, no newline rewriting, nothing
  // appended. The one thing UTF-8 cannot carry is an unpaired surrogate, which
  // the encoder replaces with U+FFFD; see the note on `countCodePoints`.
  const file = new File([text], pastedTextName(now), { type: TEXT_PASTE_MIME })
  return { kind: "upload", files: [file], pastedTextChars: chars }
}

/// How many CODE POINTS `text` holds, stopping once `cap` is reached.
///
/// CHARACTERS, not bytes and not `text.length`. Bytes would fire at a third of
/// the visible length for CJK text and a quarter for emoji, so a Japanese
/// paragraph would be filed away while an English one of the same size was
/// typed. `.length` counts UTF-16 code units, which has the same bias at half
/// strength: every emoji and every character outside the BMP counts twice. Same
/// reasoning as `pasteExceedsAttachmentLimit` in `fileDrop.ts`.
///
/// Written as a `charCodeAt` scan rather than `[...text].length`, and rather
/// than a `for...of`, for two reasons that are MEASURED on a 20-million
/// character string (node 24, this machine): the spread allocates an array of
/// 20 million one-character strings, costing 218ms and 180MB, and it does so
/// synchronously in the paste handler before anything is cancelled; the
/// iterator protocol costs 127ms and no allocation; this scan costs 37ms and no
/// allocation. With `cap` in play the ordinary under-threshold paste never gets
/// past the first thousand-odd units at all, at 0.1ms.
///
/// It agrees with `[...text].length` on every input, INCLUDING an unpaired
/// surrogate, which both count as one. That is deliberate: the two halves of
/// the feature (the threshold and the file) must not disagree about how long a
/// paste is, and the file's encoder also turns that surrogate into exactly one
/// character (U+FFFD).
export function countCodePoints(
  text: string,
  cap = Number.POSITIVE_INFINITY,
): number {
  let count = 0
  for (let i = 0; i < text.length; i++) {
    if (count >= cap) return count
    const unit = text.charCodeAt(i)
    // A high surrogate followed by a low one is ONE code point. A high
    // surrogate followed by anything else (or by nothing) is unpaired, and
    // counts as one on its own, which is what the string iterator does too.
    if (unit >= 0xd800 && unit <= 0xdbff && i + 1 < text.length) {
      const next = text.charCodeAt(i + 1)
      if (next >= 0xdc00 && next <= 0xdfff) i++
    }
    count++
  }
  return count
}
