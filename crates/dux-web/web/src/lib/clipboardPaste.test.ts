import { describe, expect, it } from "vitest"

import {
  NOT_OWNER_IMAGE_PASTE_REASON,
  NOT_OWNER_TEXT_PASTE_REASON,
  clipboardPasteAction,
  countCodePoints,
  pastedImageName,
  pastedTextName,
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
/// The ordinary case: an agent pane, held by this client, with the long-text
/// threshold at its shipped default. The image tests below never reach it,
/// because none of them pastes any text worth speaking of.
const AGENT_PANE = { kind: "agent" as const, longTextChars: 1000 }
const OWNER = {
  uploadsEnabled: true,
  isOwner: true,
  forceText: false,
  pane: AGENT_PANE,
}

describe("clipboardPasteAction", () => {
  it("leaves an empty clipboard alone", () => {
    expect(clipboardPasteAction([], "", OWNER, NOW)).toEqual({ kind: "ignore" })
  })

  it("leaves an ordinary text paste to xterm", () => {
    expect(
      clipboardPasteAction([item("string", "text/plain")], "", OWNER, NOW),
    ).toEqual({ kind: "xterm" })
  })

  it("takes an image over for upload", () => {
    const f = png("shot.png")
    const action = clipboardPasteAction([item("file", "image/png", f)], "", OWNER, NOW)
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
      "",
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
      "",
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
      "",
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
      "",
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
      "",
      { uploadsEnabled: true, isOwner: false, forceText: false, pane: AGENT_PANE },
      NOW,
    )
    expect(action).toEqual({
      kind: "refused",
      subject: "image",
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
        "",
        { uploadsEnabled: true, isOwner: false, forceText: false, pane: AGENT_PANE },
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
      clipboardPasteAction(items, "", { uploadsEnabled: false, isOwner: true, forceText: false, pane: AGENT_PANE }, NOW),
    ).toEqual({ kind: "xterm" })
    expect(
      clipboardPasteAction(
        [items[1]],
        "",
        { uploadsEnabled: false, isOwner: true, forceText: false, pane: AGENT_PANE },
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
        "",
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
        "",
        OWNER,
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
    expect(
      clipboardPasteAction([item("file", "image/png", null)], "", OWNER, NOW),
    ).toEqual({ kind: "ignore" })
  })

  it("takes the text instead of the image when the user forced a text paste", () => {
    // The escape hatch out of image-wins. Rich content (a copied spreadsheet
    // range) carries an `image/png` flavour beside the numbers, and without a
    // way out the numbers could not be pasted at all.
    expect(
      clipboardPasteAction(
        [item("string", "text/plain"), item("file", "image/png", png("s.png"))],
        "",
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
        "",
        { uploadsEnabled: true, isOwner: false, forceText: true, pane: AGENT_PANE },
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
  })

  it("ignores an image-typed STRING item", () => {
    // `kind: "string"` with an image mime type is text (an SVG source pasted
    // as markup, say), not a file.
    expect(
      clipboardPasteAction([item("string", "image/svg+xml")], "", OWNER, NOW),
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

// ── A long TEXT paste onto an agent ─────────────────────────────────────────
//
// Same journey as the image, entered by a different trigger. The reason is the
// agent's context window: a wall of pasted text spends it whether or not the
// agent needs all of it, while a path costs almost nothing and the agent reads
// what it wants.

/// An agent pane whose threshold is `chars`. A TERMINAL carries no threshold at
/// all, which is what makes the agents-only rule structural.
function agent(longTextChars = 1000) {
  return {
    uploadsEnabled: true,
    isOwner: true,
    forceText: false,
    pane: { kind: "agent" as const, longTextChars },
  }
}

function terminal() {
  return {
    uploadsEnabled: true,
    isOwner: true,
    forceText: false,
    pane: { kind: "terminal" as const },
  }
}

/// `n` characters of ordinary text, and the one clipboard item that carries it.
function long(n: number, ch = "x"): string {
  return ch.repeat(n)
}

const TEXT_ITEM = item("string", "text/plain")

async function bytesOf(file: File): Promise<Uint8Array> {
  return new Uint8Array(await file.arrayBuffer())
}

describe("a long text paste onto an agent", () => {
  it("leaves a paste at the threshold alone", () => {
    // Strictly greater, matching `pasteExceedsAttachmentLimit`: a paste of
    // exactly the threshold is still typed.
    expect(
      clipboardPasteAction([TEXT_ITEM], long(1000), agent(1000), NOW),
    ).toEqual({ kind: "xterm" })
  })

  it("saves a paste over the threshold as a .txt file and reports its size", () => {
    const text = long(1001)
    const action = clipboardPasteAction([TEXT_ITEM], text, agent(1000), NOW)
    expect(action.kind).toBe("upload")
    if (action.kind !== "upload") return
    expect(action.files.map((f) => f.name)).toEqual([
      "pasted-2026-08-09-141530.txt",
    ])
    expect(action.pastedTextChars).toBe(1001)
  })

  it("saves the pasted text byte for byte, including non-Latin text and emoji", async () => {
    // The file is the user's text and nothing else: no BOM added, no
    // normalization, no trailing newline. UTF-8, which is what the Blob
    // constructor encodes a string as and what every editor on the other end
    // will read.
    //
    // Compared as BYTES rather than through `File.text()`, and that is not
    // fussiness: `text()` decodes with a BOM sniff and STRIPS a leading U+FEFF,
    // so a paste that begins with one would fail a string comparison while the
    // file on disk was perfectly correct. Bytes answer the question actually
    // being asked.
    const text = `${"日本語のテキスト".repeat(200)}\n🙂🙂🙂\r\ntail`
    const action = clipboardPasteAction([TEXT_ITEM], text, agent(200), NOW)
    expect(action.kind).toBe("upload")
    if (action.kind !== "upload") return
    expect(await bytesOf(action.files[0])).toEqual(
      new TextEncoder().encode(text),
    )
  })

  it("keeps every awkward character a JavaScript string can hold", async () => {
    // The shapes that a naive implementation loses, each of which was MEASURED
    // to survive: a CRLF, a lone CR, trailing whitespace, a NUL, an ESC, a
    // combining mark (which normalization would fold), an RTL mark, and a
    // leading U+FEFF that arrives as content and must stay content.
    const text =
      "\ufeffa\r\nb\rc   \n\u0000\u001b[31me\u0301\u200fx\t \n"
    const action = clipboardPasteAction([TEXT_ITEM], text, agent(1), NOW)
    expect(action.kind).toBe("upload")
    if (action.kind !== "upload") return
    expect(await bytesOf(action.files[0])).toEqual(
      new TextEncoder().encode(text),
    )
  })

  it("turns a LONE SURROGATE into U+FFFD, which is the one thing it cannot keep", async () => {
    // MEASURED, and pinned here so it is a known property rather than a
    // surprise. A JavaScript string can hold an unpaired surrogate; UTF-8
    // cannot represent one, so the `Blob` constructor's encoder substitutes
    // U+FFFD (EF BF BD). It still counts as ONE code point, so it neither hides
    // from the threshold nor inflates it.
    //
    // It is not worth defending against: a lone surrogate reaches a paste event
    // only from a source that already produced broken text, and the
    // alternatives (refuse the paste, or invent an encoding) are both worse
    // than the replacement character every other tool in the chain would also
    // produce.
    const text = `${"x".repeat(10)}\uD800tail`
    expect(countCodePoints(text)).toBe(15)
    const action = clipboardPasteAction([TEXT_ITEM], text, agent(1), NOW)
    expect(action.kind).toBe("upload")
    if (action.kind !== "upload") return
    expect(await bytesOf(action.files[0])).toEqual(
      new Uint8Array([
        ...new TextEncoder().encode("x".repeat(10)),
        0xef,
        0xbf,
        0xbd,
        ...new TextEncoder().encode("tail"),
      ]),
    )
  })

  it("measures the threshold in CHARACTERS, not UTF-16 units and not bytes", () => {
    // 900 emoji: 900 characters, 1800 UTF-16 code units, 3600 UTF-8 bytes. A
    // threshold of 1000 must not fire, or an emoji-heavy or CJK paste would be
    // filed away at a third of the length an ASCII one is allowed.
    const emoji = long(900, "🙂")
    expect([...emoji].length).toBe(900)
    expect(emoji.length).toBe(1800)
    expect(new TextEncoder().encode(emoji).length).toBe(3600)
    expect(clipboardPasteAction([TEXT_ITEM], emoji, agent(1000), NOW)).toEqual({
      kind: "xterm",
    })
    // And it does fire once the CHARACTER count is over.
    expect(
      clipboardPasteAction([TEXT_ITEM], long(1001, "🙂"), agent(1000), NOW).kind,
    ).toBe("upload")
  })

  it("pastes long text verbatim when the user forced a text paste", () => {
    // The one hatch, shared with image-wins: Ctrl+Shift+v means "give it to me
    // literally", so it beats this too rather than needing a second chord.
    expect(
      clipboardPasteAction(
        [TEXT_ITEM],
        long(50_000),
        { ...agent(1000), forceText: true },
        NOW,
      ),
    ).toEqual({ kind: "xterm" })
  })

  it("pastes long text verbatim into a TERMINAL, at any length", () => {
    // Structural, not a condition: the terminal variant of the pane carries no
    // threshold to read. A long paste into a shell is a command or a heredoc,
    // and turning it into a file would destroy what the user meant.
    expect(
      clipboardPasteAction([TEXT_ITEM], long(50_000), terminal(), NOW),
    ).toEqual({ kind: "xterm" })
  })

  it("is switched off by a threshold of 0", () => {
    expect(
      clipboardPasteAction([TEXT_ITEM], long(50_000), agent(0), NOW),
    ).toEqual({ kind: "xterm" })
  })

  it("does nothing with a long text paste while uploads are switched off", () => {
    expect(
      clipboardPasteAction([TEXT_ITEM], long(50_000), {
        ...agent(1000),
        uploadsEnabled: false,
      }, NOW),
    ).toEqual({ kind: "xterm" })
  })

  it("refuses a long text paste from a client that does not hold input, and saves nothing", () => {
    const action = clipboardPasteAction(
      [TEXT_ITEM],
      long(1001),
      { ...agent(1000), isOwner: false },
      NOW,
    )
    expect(action).toEqual({
      kind: "refused",
      // Its OWN subject, so the two refusals cannot land on one toast id and
      // replace each other: an image refusal and a text refusal are two
      // different things that went wrong.
      subject: "text",
      reason: NOT_OWNER_TEXT_PASTE_REASON,
    })
  })

  it("still prefers the image when the same paste carries both", () => {
    const action = clipboardPasteAction(
      [TEXT_ITEM, item("file", "image/png", png("shot.png"))],
      long(50_000),
      agent(1000),
      NOW,
    )
    expect(action.kind).toBe("upload")
    if (action.kind !== "upload") return
    expect(action.files.map((f) => f.name)).toEqual(["shot.png"])
    expect(action.pastedTextChars).toBeUndefined()
  })
})

describe("pastedTextName", () => {
  it("is the pasted-image name shape with a .txt extension", () => {
    expect(pastedTextName(NOW)).toBe("pasted-2026-08-09-141530.txt")
  })

  it("pads every clock component so the names sort", () => {
    expect(pastedTextName(new Date(2026, 0, 2, 3, 4, 5))).toBe(
      "pasted-2026-01-02-030405.txt",
    )
  })
})
