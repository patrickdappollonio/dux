// Pasting an image out of the clipboard onto an agent or a terminal: the pure
// decision, and the name an unnamed clipboard image is saved under.
//
// The gesture this exists for is the ordinary one: take a screenshot, press
// paste, hand the picture to the agent. Dropping a file already did this
// (`fileDrop.ts`), and this is the same journey through the same machinery,
// entered by a different gesture. Nothing here uploads, writes, or touches a
// terminal; the component applies the decision and reuses the drop path.
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
  forceText: boolean
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
  | { kind: "upload"; files: File[] }
  | { kind: "refused"; reason: string }

/// Why an image paste is refused for a client that is only watching, as the
/// whole sentence the toast shows. It says the outcome first (nothing was
/// saved, so there is no stray file to hunt for) and then the way out, because
/// taking over is one tap away.
export const NOT_OWNER_IMAGE_PASTE_REASON =
  "The image was not saved: another device is driving this session. Take over to paste it here."

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

function two(n: number): string {
  return String(n).padStart(2, "0")
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
  const stamp =
    `${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}` +
    `-${two(now.getHours())}${two(now.getMinutes())}${two(now.getSeconds())}`
  const ext = IMAGE_EXTENSIONS[type.toLowerCase()] ?? FALLBACK_IMAGE_EXTENSION
  return `pasted-${stamp}.${ext}`
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
export function clipboardPasteAction(
  items: readonly ClipboardItemLike[],
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
  if (files.length === 0) return fallback
  if (!ctx.isOwner) {
    return { kind: "refused", reason: NOT_OWNER_IMAGE_PASTE_REASON }
  }
  return { kind: "upload", files }
}
