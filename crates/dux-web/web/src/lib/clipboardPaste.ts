// The pure decision for a paste onto an agent or a terminal, and the names dux
// invents for an unnamed clipboard image and a filed-away long text paste.
// Nothing here uploads or writes; the caller reuses the drop path in
// `fileDrop.ts`, so both triggers end as a saved file whose path is pasted.
//
// A long text paste becomes a file because a wall of text spends an agent's
// context whether or not it needs all of it, while a path costs almost nothing.
// The decision reads the `paste` event rather than `navigator.clipboard.read()`,
// which is blocked outside a secure context, and dux is routinely served over
// plain HTTP.

/// A `DataTransferItem`, structurally: the real type cannot be constructed in a
/// test, so only the members this decision reads are named.
export type ClipboardItemLike = {
  readonly kind: string
  readonly type: string
  getAsFile: () => File | null
}

/// What is being pasted into. A union rather than a flag so the agents-only rule
/// is structural: a terminal carries no threshold to invert. A long paste into a
/// shell is usually a command or a heredoc, so a terminal pastes text verbatim
/// at any length.
export type ClipboardPastePane =
  /// An agent pane. `longTextChars` is `ui.upload_pasted_text_chars`: a text
  /// paste longer than this many characters is saved as a file instead of being
  /// typed. `0` switches it off, and so does an absent field from an older
  /// server, which the browser reads as `0`.
  | { kind: "agent"; longTextChars: number }
  /// A terminal pane. No threshold, on purpose. See above.
  | { kind: "terminal" }

/// The runtime facts the decision folds in beside the clipboard's own contents.
export type ClipboardPasteContext = {
  /// Whether uploads exist at all on this server (`[server] file_drop_max_bytes`
  /// above zero). When they do not, every paste is left to xterm.
  uploadsEnabled: boolean
  /// Whether this client holds input for the pane being pasted into.
  isOwner: boolean
  /// Whether the user asked for a text paste with the `Ctrl+Shift+v` /
  /// `Cmd+Shift+v` chord (`forcesTextPaste` in `termkeys.ts`). One hatch for
  /// both triggers: it skips image handling, beats the long-text rule, and
  /// leaves the paste to xterm, so rich content's text stays reachable.
  forceText: boolean
  /// Which kind of pane this paste landed on. See [`ClipboardPastePane`].
  pane: ClipboardPastePane
}

/// What the pane must do with one `paste` event. `refused` is separate from
/// `ignore` because a paste a watcher cannot make must save nothing and say so.
/// `ignore` and `xterm` are the same inaction today and stay apart because they
/// say different things: no text at all, versus text that belongs to xterm.
export type ClipboardPasteAction =
  | { kind: "ignore" }
  | { kind: "xterm" }
  | {
      kind: "upload"
      files: File[]
      /// Set only when these files are one long text paste dux turned into a
      /// document, and then it is that paste's character count, which the toast
      /// leads with. Absent for an image paste.
      pastedTextChars?: number
    }
  | {
      kind: "refused"
      /// What was turned away, so the caller can give the two refusals two toast
      /// ids; on one shared id the second silently replaces the first.
      subject: "image" | "text"
      reason: string
    }

/// Why an image paste is refused for a watcher, as the whole sentence the toast
/// shows: the outcome first, so there is no stray file to hunt for, then the way
/// out, because taking over is one tap away.
export const NOT_OWNER_IMAGE_PASTE_REASON =
  "The image was not saved: another device is driving this session. Take over to paste it here."

/// The same, for a long text paste that would have become a file. Its own
/// sentence: "the image was not saved" would be a lie about a paragraph of text.
export const NOT_OWNER_TEXT_PASTE_REASON =
  "The pasted text was not saved: another device is driving this session. Take over to paste it here."

/// The extension for an image the clipboard handed over with no name, keyed by
/// mime type. Explicit rather than derived from the subtype, which disagrees
/// where it matters: `image/jpeg` is `.jpg` and `image/svg+xml` is `.svg`.
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

/// The extension used when the mime type is unknown or missing. Not derived
/// from the type string, whose subtype can carry characters no file name may
/// hold: the server validates names rather than rewriting them.
const FALLBACK_IMAGE_EXTENSION = "img"

/// The extension a filed-away text paste gets. Plain text, so the user, the
/// agent and any editor all read the same thing.
const TEXT_PASTE_EXTENSION = "txt"

/// The mime type the synthesised text file carries. The charset is spelled out
/// because `Blob` encodes the string as UTF-8, which is what makes the file a
/// byte-for-byte copy of the clipboard.
const TEXT_PASTE_MIME = "text/plain;charset=utf-8"

function two(n: number): string {
  return String(n).padStart(2, "0")
}

/// The `pasted-<local clock>` stem both synthesised names share. Local time, not
/// UTC: the user reading the folder is on the clock they pressed paste by. The
/// moment is a parameter so the names can be pinned by tests.
function pastedStem(now: Date): string {
  return (
    `pasted-${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}` +
    `-${two(now.getHours())}${two(now.getMinutes())}${two(now.getSeconds())}`
  )
}

/// What to call a pasted image. The clipboard's own name wins whenever there is
/// one, as it is the user's; collisions are left to the server's suffix. Only a
/// missing name is invented, from the clock, so two pastes are distinguishable
/// in a folder listing.
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

/// What to call the file a long text paste becomes. The same shape as
/// [`pastedImageName`]'s invented name: they land in the same folder and a
/// reader scanning it should see one convention. Text carries no name to prefer.
export function pastedTextName(now: Date): string {
  return `${pastedStem(now)}.${TEXT_PASTE_EXTENSION}`
}

/// Whether one clipboard item is an image file. Both halves are required: an
/// `image/svg+xml` item of kind `string` is markup copied out of an editor, and
/// belongs in the prompt as text.
function isImageFile(item: ClipboardItemLike): boolean {
  return item.kind === "file" && item.type.toLowerCase().startsWith("image/")
}

/// What the pane should do with the contents of one `paste` event.
///
/// An image wins over text in the same event: a copied screenshot routinely
/// carries a `text/html` snapshot beside it, and letting both through would
/// paste the path and then dump markup after it. `ctx.forceText` is the way
/// past that. A non-image file is never accepted, being usually an artifact of
/// how an application puts rich content on the clipboard. Everything else is
/// left to xterm's own paste handler.
///
/// `text` is the clipboard's `text/plain` flavour, read synchronously by the
/// caller: a `DataTransferItem` of kind `string` yields only through an async
/// callback, and the decision must be made before the `paste` event finishes
/// dispatching or there is no cancelling it. Empty string when there is none.
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
  // Before the ownership gate: a forced text paste has refused nothing, so a
  // watcher using the hatch must not be told an image was turned away.
  if (ctx.forceText) return fallback

  // Resolved before the ownership gate, so a clipboard carrying no usable image
  // is never reported to a watcher as a refusal.
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

  // No image. A long enough text paste onto an agent becomes a document. After
  // the image, so a paste carrying both still prefers the image.
  return longTextPasteAction(text, ctx, now) ?? fallback
}

/// The long-text half of the decision, or `null` when this paste is not one.
function longTextPasteAction(
  text: string,
  ctx: ClipboardPasteContext,
  now: Date,
): ClipboardPasteAction | null {
  // A terminal has no threshold to read, by construction.
  if (ctx.pane.kind !== "agent") return null
  const limit = ctx.pane.longTextChars
  // `0` is the documented "switch this off" value, and it is also what the
  // browser sees from a server too old to publish the setting.
  if (limit <= 0) return null
  // Counting stops at `limit + 1`, so an ordinary paste costs O(limit) inside
  // the paste handler. Strictly greater: exactly the threshold is still typed.
  if (countCodePoints(text, limit + 1) <= limit) return null
  if (!ctx.isOwner) {
    return {
      kind: "refused",
      subject: "text",
      reason: NOT_OWNER_TEXT_PASTE_REASON,
    }
  }
  // Only now is the exact size worth a full pass: the toast reports it, and it
  // is the number the user acts on when setting the threshold.
  const chars = countCodePoints(text)
  // `Blob` encodes the string as UTF-8, so the file is the pasted string byte
  // for byte, except an unpaired surrogate, which becomes U+FFFD.
  const file = new File([text], pastedTextName(now), { type: TEXT_PASTE_MIME })
  return { kind: "upload", files: [file], pastedTextChars: chars }
}

/// How many code points `text` holds, stopping once `cap` is reached.
///
/// Code points, not bytes and not `text.length`: both are biased against CJK and
/// emoji, so a Japanese paragraph would be filed away while an English one of
/// the same visible length was typed. Same rule as `pasteExceedsAttachmentLimit`
/// in `fileDrop.ts`.
///
/// A `charCodeAt` scan rather than `[...text].length`, which allocates a string
/// per character inside the paste handler and measured several times slower. It
/// agrees with the spread on every input, unpaired surrogates included, so the
/// threshold and the file cannot disagree about how long a paste is.
export function countCodePoints(
  text: string,
  cap = Number.POSITIVE_INFINITY,
): number {
  let count = 0
  for (let i = 0; i < text.length; i++) {
    if (count >= cap) return count
    const unit = text.charCodeAt(i)
    // A high surrogate followed by a low one is one code point; an unpaired high
    // surrogate counts as one on its own, which the string iterator does too.
    if (unit >= 0xd800 && unit <= 0xdbff && i + 1 < text.length) {
      const next = text.charCodeAt(i + 1)
      if (next >= 0xdc00 && next <= 0xdfff) i++
    }
    count++
  }
  return count
}
