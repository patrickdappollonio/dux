import { describe, expect, it } from "vitest"

import {
  NOT_OWNER_IMAGE_PASTE_REASON,
  clipboardPasteAction,
  pastedImageName,
  type ClipboardItemLike,
} from "./clipboardPaste"

/// A `DataTransferItem` stand-in. The real thing cannot be constructed, and the
/// decision only ever reads these three members.
function item(
  kind: string,
  type: string,
  file: File | null = null,
): ClipboardItemLike {
  return { kind, type, getAsFile: () => file }
}

function png(name: string): File {
  return new File([new Uint8Array([1, 2, 3])], name, { type: "image/png" })
}

const NOW = new Date(2026, 7, 9, 14, 15, 30)
const OWNER = { uploadsEnabled: true, isOwner: true, forceText: false }

describe("clipboardPasteAction", () => {
  it("leaves an empty clipboard alone", () => {
    expect(clipboardPasteAction([], OWNER, NOW)).toEqual({ kind: "ignore" })
  })

  it("leaves an ordinary text paste to xterm", () => {
    expect(
      clipboardPasteAction([item("string", "text/plain")], OWNER, NOW),
    ).toEqual({ kind: "xterm" })
  })

  it("takes an image over for upload", () => {
    const f = png("shot.png")
    const action = clipboardPasteAction([item("file", "image/png", f)], OWNER, NOW)
    expect(action.kind).toBe("upload")
    if (action.kind !== "upload") return
    expect(action.files.map((x) => x.name)).toEqual(["shot.png"])
  })

  it("takes the image when the same paste also carries text", () => {
    // A screenshot copied out of an app routinely arrives as image/png PLUS a
    // text/html snapshot. The image is what the user meant; letting the text
    // through as well would dump markup into the prompt beside the path.
    const f = png("shot.png")
    const action = clipboardPasteAction(
      [
        item("string", "text/plain"),
        item("string", "text/html"),
        item("file", "image/png", f),
      ],
      OWNER,
      NOW,
    )
    expect(action.kind).toBe("upload")
  })

  it("keeps several images in clipboard order", () => {
    const a = png("a.png")
    const b = png("b.png")
    const action = clipboardPasteAction(
      [item("file", "image/png", a), item("file", "image/png", b)],
      OWNER,
      NOW,
    )
    expect(action.kind === "upload" && action.files.map((f) => f.name)).toEqual([
      "a.png",
      "b.png",
    ])
  })

  it("names an unnamed image after the moment it was pasted", () => {
    const action = clipboardPasteAction(
      [item("file", "image/png", new File([], "", { type: "image/png" }))],
      OWNER,
      NOW,
    )
    expect(action.kind === "upload" && action.files[0].name).toBe(
      "pasted-2026-08-09-141530.png",
    )
  })

  it("keeps a non-Latin name exactly as the clipboard gave it", () => {
    const action = clipboardPasteAction(
      [item("file", "image/png", png("スクリーンショット.png"))],
      OWNER,
      NOW,
    )
    expect(action.kind === "upload" && action.files[0].name).toBe(
      "スクリーンショット.png",
    )
  })

  it("refuses an image paste from a client that does not hold input", () => {
    const action = clipboardPasteAction(
      [item("file", "image/png", png("shot.png"))],
      { uploadsEnabled: true, isOwner: false, forceText: false },
      NOW,
    )
    expect(action).toEqual({
      kind: "refused",
      reason: NOT_OWNER_IMAGE_PASTE_REASON,
    })
  })

  it("leaves a text paste alone for a non-owner, because xterm already gates it", () => {
    // Only the IMAGE half is refused here. Text is xterm's business and its
    // write is gated on the socket anyway, so nothing is gained by
    // intercepting it and a refusal toast would be a lie about what happened.
    expect(
      clipboardPasteAction(
        [item("string", "text/plain")],
        { uploadsEnabled: true, isOwner: false, forceText: false },
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
  })

  it("does nothing special with an image while uploads are switched off", () => {
    // `[server] file_drop_max_bytes = 0` switches the whole upload feature off.
    // The paste then behaves exactly as it did before this feature existed.
    const items = [
      item("string", "text/plain"),
      item("file", "image/png", png("shot.png")),
    ]
    expect(
      clipboardPasteAction(items, { uploadsEnabled: false, isOwner: true, forceText: false }, NOW),
    ).toEqual({ kind: "xterm" })
    expect(
      clipboardPasteAction(
        [items[1]],
        { uploadsEnabled: false, isOwner: true, forceText: false },
        NOW,
      ),
    ).toEqual({ kind: "ignore" })
  })

  it("leaves a non-image file item alone", () => {
    // Deliberately narrow. The gesture this exists for is a screenshot, and a
    // `kind: "file"` item that is not an image is usually an artifact of how
    // an app puts rich content on the clipboard, where hijacking the paste
    // would be wrong.
    const pdf = new File([new Uint8Array([1])], "a.pdf", {
      type: "application/pdf",
    })
    expect(
      clipboardPasteAction(
        [item("string", "text/plain"), item("file", "application/pdf", pdf)],
        OWNER,
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
  })

  it("falls back to the text path when an image item yields no file", () => {
    // `getAsFile()` is documented to return null, and an image item that
    // cannot produce bytes is not an image paste.
    expect(
      clipboardPasteAction(
        [item("string", "text/plain"), item("file", "image/png", null)],
        OWNER,
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
    expect(
      clipboardPasteAction([item("file", "image/png", null)], OWNER, NOW),
    ).toEqual({ kind: "ignore" })
  })

  it("takes the text instead of the image when the user forced a text paste", () => {
    // The escape hatch out of image-wins. Rich content (a copied spreadsheet
    // range) carries an `image/png` flavour beside the numbers, and without a
    // way out the numbers could not be pasted at all.
    expect(
      clipboardPasteAction(
        [item("string", "text/plain"), item("file", "image/png", png("s.png"))],
        { ...OWNER, forceText: true },
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
  })

  it("does not report a refusal to a non-owner who forced a text paste", () => {
    // Nothing was turned away, so nothing must be said. Ordering matters: the
    // hatch is checked BEFORE the ownership gate for exactly this reason.
    expect(
      clipboardPasteAction(
        [item("string", "text/plain"), item("file", "image/png", png("s.png"))],
        { uploadsEnabled: true, isOwner: false, forceText: true },
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
  })

  it("ignores an image-typed STRING item", () => {
    // `kind: "string"` with an image mime type is text (an SVG source pasted
    // as markup, say), not a file.
    expect(
      clipboardPasteAction([item("string", "image/svg+xml")], OWNER, NOW),
    ).toEqual({ kind: "xterm" })
  })
})

describe("pastedImageName", () => {
  it("keeps whatever name the clipboard supplied", () => {
    expect(pastedImageName("Screen Shot.png", "image/png", NOW)).toBe(
      "Screen Shot.png",
    )
  })

  it("names an unnamed file from the clock and the mime type", () => {
    expect(pastedImageName("", "image/png", NOW)).toBe(
      "pasted-2026-08-09-141530.png",
    )
    expect(pastedImageName("   ", "image/jpeg", NOW)).toBe(
      "pasted-2026-08-09-141530.jpg",
    )
    expect(pastedImageName("", "image/webp", NOW)).toBe(
      "pasted-2026-08-09-141530.webp",
    )
    expect(pastedImageName("", "image/svg+xml", NOW)).toBe(
      "pasted-2026-08-09-141530.svg",
    )
  })

  it("falls back to a safe extension for a mime type it does not know", () => {
    expect(pastedImageName("", "image/x-weird thing", NOW)).toBe(
      "pasted-2026-08-09-141530.img",
    )
    expect(pastedImageName("", "", NOW)).toBe("pasted-2026-08-09-141530.img")
  })

  it("pads every clock component so the names sort", () => {
    expect(pastedImageName("", "image/png", new Date(2026, 0, 2, 3, 4, 5))).toBe(
      "pasted-2026-01-02-030405.png",
    )
  })
})
