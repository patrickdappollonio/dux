import { describe, expect, it } from "vitest"

import {
  MAX_NAMED_FILES,
  dropToastFor,
  pastePayload,
  quoteShellToken,
  type DropOutcome,
} from "./fileDrop"

const agent = { kind: "agent" as const, folderLabel: "~/code/app/wt" }
const terminal = { kind: "terminal" as const, folderLabel: "~/code/app" }

function pasted(name: string, saved = name): DropOutcome {
  return {
    kind: "pasted",
    requestedName: name,
    savedName: saved,
    path: `/home/p/code/app/${saved}`,
  }
}

describe("quoteShellToken", () => {
  it("wraps the whole path as one token so a directory with spaces survives", () => {
    // The case the first design missed: the FILENAME is only the last part. A
    // worktree path is built from the project's name, and "Web App" is a
    // project name in dux's own tests.
    expect(quoteShellToken("/home/p/Web App/shot.png")).toBe(
      "'/home/p/Web App/shot.png'",
    )
  })

  it("survives a directory containing quotes and shell metacharacters", () => {
    expect(quoteShellToken("/tmp/it's a \"dir\"/x.png")).toBe(
      "'/tmp/it'\\''s a \"dir\"/x.png'",
    )
    expect(quoteShellToken("/tmp/$(rm -rf ~)/x.png")).toBe(
      "'/tmp/$(rm -rf ~)/x.png'",
    )
    expect(quoteShellToken("/tmp/a;b|c&d/x.png")).toBe("'/tmp/a;b|c&d/x.png'")
  })

  it("keeps a non-Latin name intact", () => {
    expect(quoteShellToken("/tmp/スクリーンショット.png")).toBe(
      "'/tmp/スクリーンショット.png'",
    )
  })
})

describe("pastePayload", () => {
  it("is the quoted path, one trailing space, and NO newline", () => {
    // A newline submits in these tools, so a file arriving with an automatic
    // submit would fire a half-written prompt. This is the assertion that
    // pins it.
    const payload = pastePayload("/home/p/shot.png")
    expect(payload).toBe("'/home/p/shot.png' ")
    expect(payload).not.toContain("\n")
    expect(payload).not.toContain("\r")
    expect(payload.endsWith(" ")).toBe(true)
  })
})

describe("dropToastFor", () => {
  it("names the file and the folder for a single success", () => {
    const t = dropToastFor([pasted("shot.png")], terminal)
    expect(t.tone).toBe("success")
    expect(t.message).toContain("shot.png")
    expect(t.message).toContain("~/code/app")
  })

  it("describes an agent's destination as the worktree root, not a long path", () => {
    const t = dropToastFor([pasted("shot.png")], agent)
    expect(t.tone).toBe("success")
    expect(t.message).toContain("worktree root")
  })

  it("gives a count rather than a list for several successes", () => {
    const t = dropToastFor(
      [pasted("a.png"), pasted("b.png"), pasted("c.png")],
      terminal,
    )
    expect(t.tone).toBe("success")
    expect(t.message).toContain("3 files")
  })

  it("lists a rename as original and saved name, never as a count", () => {
    // A count tells the user something changed without telling them what the
    // file is now called, which defeats the point of reporting it at all.
    const t = dropToastFor([pasted("shot.png", "shot-S-1.png")], terminal)
    expect(t.tone).toBe("success")
    expect(t.message).toContain("shot.png")
    expect(t.message).toContain("shot-S-1.png")
    expect(t.message).not.toMatch(/1 file renamed/)
  })

  it("falls back to naming the folder when there are too many renames to list", () => {
    const many = Array.from({ length: MAX_NAMED_FILES + 2 }, (_, i) =>
      pasted(`shot${i}.png`, `shot${i}-S-1.png`),
    )
    const t = dropToastFor(many, terminal)
    expect(t.message).toContain("~/code/app")
    expect(t.message).not.toContain("shot0-S-1.png")
  })

  it("is an error when nothing was saved, and gives the reasons", () => {
    const t = dropToastFor(
      [
        {
          kind: "refused",
          requestedName: "big.png",
          reason: "over the size limit",
        },
      ],
      terminal,
    )
    expect(t.tone).toBe("error")
    expect(t.message).toContain("big.png")
    expect(t.message).toContain("over the size limit")
  })

  it("warns, naming the file and its FULL path, when a save could not be pasted", () => {
    // The user has to be able to reach the file by hand, so the full path is
    // required here and a shortened folder is not enough.
    const t = dropToastFor(
      [
        {
          kind: "saved-not-sent",
          requestedName: "shot.png",
          savedName: "shot.png",
          path: "/home/p/code/app/shot.png",
          reason: "another device is driving this terminal",
        },
      ],
      terminal,
    )
    expect(t.tone).toBe("warning")
    expect(t.message).toContain("/home/p/code/app/shot.png")
    expect(t.message).toContain("another device is driving this terminal")
  })

  it("lets the worse outcome win over successes, at every rung", () => {
    // A bad outcome must never be reported as a good one, so each rung is
    // checked MIXED with a success rather than alone.
    const notSent: DropOutcome = {
      kind: "saved-not-sent",
      requestedName: "b.png",
      savedName: "b.png",
      path: "/home/p/code/app/b.png",
      reason: "the connection dropped",
    }
    const refused: DropOutcome = {
      kind: "refused",
      requestedName: "c.png",
      reason: "over the size limit",
    }

    expect(dropToastFor([pasted("a.png"), notSent], terminal).tone).toBe(
      "warning",
    )
    expect(dropToastFor([pasted("a.png"), refused], terminal).tone).toBe(
      "warning",
    )
    // Not-sent outranks refused: it is the one with a file the user now has to
    // find by hand.
    const both = dropToastFor([pasted("a.png"), notSent, refused], terminal)
    expect(both.tone).toBe("warning")
    expect(both.message).toContain("/home/p/code/app/b.png")
    // ...and the refusal is still reported, because dropping it would leave the
    // user believing that file went somewhere.
    expect(both.message).toContain("c.png")
  })

  it("says what saved and what did not when refusals sit alongside successes", () => {
    const t = dropToastFor(
      [
        pasted("a.png"),
        pasted("b.png"),
        { kind: "refused", requestedName: "c.png", reason: "an unusable name" },
      ],
      terminal,
    )
    expect(t.tone).toBe("warning")
    expect(t.message).toContain("2 of 3")
    expect(t.message).toContain("c.png")
    expect(t.message).toContain("an unusable name")
  })

  it("reports every refusal reason it was given rather than a generic one", () => {
    for (const reason of [
      "over the size limit",
      "an unusable name",
      "the destination is not writable",
      "another device is driving this terminal",
    ]) {
      const t = dropToastFor(
        [{ kind: "refused", requestedName: "x.png", reason }],
        terminal,
      )
      expect(t.message).toContain(reason)
    }
  })
})
