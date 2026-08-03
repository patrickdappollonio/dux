import { describe, expect, it } from "vitest"

import {
  MAX_NAMED_FILES,
  dropToastFor,
  pastePayload,
  type DropOutcome,
} from "./fileDrop"

const agent = { kind: "agent" as const }
const terminal = { kind: "terminal" as const }

function sent(
  name: string,
  saved = name,
  folderLabel = "~/code/app",
): DropOutcome {
  return {
    kind: "sent",
    requestedName: name,
    savedName: saved,
    path: `/home/p/code/app/${saved}`,
    folderLabel,
  }
}

function notSent(
  name: string,
  saved = name,
  folderLabel = "~/code/app",
  reason = "the connection dropped",
): DropOutcome {
  return {
    kind: "saved-not-sent",
    requestedName: name,
    savedName: saved,
    path: `/home/p/code/app/${saved}`,
    folderLabel,
    reason,
  }
}

describe("pastePayload", () => {
  it("is the BARE path, one trailing space, and NO newline", () => {
    // A newline submits in these tools, so a file arriving with an automatic
    // submit would fire a half-written prompt. This is the assertion that
    // pins it.
    const payload = pastePayload("/home/p/shot.png")
    expect(payload).toBe("/home/p/shot.png ")
    expect(payload).not.toContain("\n")
    expect(payload).not.toContain("\r")
    expect(payload.endsWith(" ")).toBe(true)
  })

  it("adds nothing at all to an awkward path", () => {
    // The whole point of the reversal: whatever is on disk is what goes out.
    // The byte-for-byte proof lives in the pane's own tests, at the socket;
    // this is the same rule stated where the payload is built.
    for (const path of [
      "/home/p/Web App/shot.png",
      "/home/p/Bob's app/shot.png",
      "/tmp/$(rm -rf ~)/x.png",
      "/tmp/`whoami`/x.png",
      '/tmp/it"s/x.png',
      "/tmp/スクリーンショット.png",
    ]) {
      expect(pastePayload(path)).toBe(`${path} `)
    }
  })
})

describe("dropToastFor", () => {
  it("names the file and the folder for a single success", () => {
    const t = dropToastFor([sent("shot.png")], terminal)
    expect(t.tone).toBe("success")
    expect(t.message).toContain("shot.png")
    expect(t.message).toContain("~/code/app")
  })

  it("describes an agent's destination as the worktree root, not a long path", () => {
    const t = dropToastFor([sent("shot.png")], agent)
    expect(t.tone).toBe("success")
    expect(t.message).toContain("worktree root")
  })

  it("gives a count rather than a list for several successes", () => {
    const t = dropToastFor(
      [sent("a.png"), sent("b.png"), sent("c.png")],
      terminal,
    )
    expect(t.tone).toBe("success")
    expect(t.message).toContain("3 files")
  })

  it("lists a rename as original and saved name, never as a count", () => {
    // A count tells the user something changed without telling them what the
    // file is now called, which defeats the point of reporting it at all.
    const t = dropToastFor([sent("shot.png", "shot-S-1.png")], terminal)
    expect(t.tone).toBe("success")
    expect(t.message).toContain("shot.png")
    expect(t.message).toContain("shot-S-1.png")
    expect(t.message).not.toMatch(/1 file renamed/)
  })

  it("falls back to naming the folder when there are too many renames to list", () => {
    const many = Array.from({ length: MAX_NAMED_FILES + 2 }, (_, i) =>
      sent(`shot${i}.png`, `shot${i}-S-1.png`),
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
        notSent(
          "shot.png",
          "shot.png",
          "~/code/app",
          "another device is driving this terminal",
        ),
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
    const stranded = notSent("b.png")
    const refused: DropOutcome = {
      kind: "refused",
      requestedName: "c.png",
      reason: "over the size limit",
    }

    expect(dropToastFor([sent("a.png"), stranded], terminal).tone).toBe(
      "warning",
    )
    expect(dropToastFor([sent("a.png"), refused], terminal).tone).toBe(
      "warning",
    )
    // Not-sent outranks refused: it is the one with a file the user now has to
    // find by hand.
    const both = dropToastFor([sent("a.png"), stranded, refused], terminal)
    expect(both.tone).toBe("warning")
    expect(both.message).toContain("/home/p/code/app/b.png")
    // ...and the refusal is still reported, because dropping it would leave the
    // user believing that file went somewhere.
    expect(both.message).toContain("c.png")
  })

  it("says what saved and what did not when refusals sit alongside successes", () => {
    const t = dropToastFor(
      [
        sent("a.png"),
        sent("b.png"),
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

describe("a drop whose files did not all end the same way", () => {
  // The four defects this covers, with the values the old code produced:
  //
  //  - The saved-but-not-sent rung never added the rename note, so a mixed drop
  //    reported only "Saved to ~/second, but the path was not sent: ...", losing
  //    a.png -> a-1.png entirely, exactly when the user has to find it by hand.
  //  - Renames among the SENT files were dropped the moment anything else was
  //    not sent, for the same reason.
  //  - Only the last successful upload's folder was kept, so two files in two
  //    folders read "Saved 2 files to ~/second", which was false for the first.
  //  - Success said "pasted their paths", which dux cannot know: a take-over
  //    between the courtesy check and the socket frame makes the server drop it
  //    with no acknowledgement.
  const mixed: DropOutcome[] = [
    sent("a.png", "a-1.png", "~/first"),
    notSent("b.png", "b-1.png", "~/second"),
    { kind: "refused", requestedName: "c.png", reason: "over the size limit" },
  ]

  it("keeps every rename, whichever rung the toast lands on", () => {
    const t = dropToastFor(mixed, terminal)
    expect(t.tone).toBe("warning")
    expect(t.message).toContain("a.png was saved as a-1.png")
    expect(t.message).toContain("b.png was saved as b-1.png")
  })

  it("never claims one folder for files that went to two", () => {
    const t = dropToastFor(mixed, terminal)
    expect(t.message).toContain("a-1.png to ~/first")
    expect(t.message).toContain("b-1.png to ~/second")
    // The specific falsehood: a blanket "to <one folder>" clause.
    expect(t.message).not.toContain("Saved to ~/second,")
    expect(t.message).not.toMatch(/Saved \d+ files to ~\//)
  })

  it("still says the folder plainly when every file did go to one", () => {
    // The breakdown is for the case that needs it. One folder must not be
    // dressed up as a list.
    const t = dropToastFor(
      [sent("a.png", "a.png", "~/one"), sent("b.png", "b.png", "~/one")],
      terminal,
    )
    expect(t.message).toContain("Saved 2 files to ~/one")
    expect(t.message).not.toContain("did not all land together")
  })

  it("groups the breakdown by folder rather than listing every file", () => {
    const t = dropToastFor(
      [
        sent("a.png", "a.png", "~/one"),
        sent("b.png", "b.png", "~/one"),
        sent("c.png", "c.png", "~/two"),
      ],
      terminal,
    )
    expect(t.message).toContain("a.png and b.png to ~/one")
    expect(t.message).toContain("c.png to ~/two")
  })

  it("an agent's files are described by the worktree root and never broken down", () => {
    // Every tab of one agent shares one worktree, so there is nothing to break
    // down even though each outcome carries a label.
    const t = dropToastFor(
      [sent("a.png", "a.png", "~/wt"), sent("b.png", "b.png", "~/wt")],
      agent,
    )
    expect(t.message).toContain("the agent's worktree root")
    expect(t.message).not.toContain("did not all land together")
  })

  it("claims only that the path was SENT, never that it was pasted", () => {
    // A write to the PTY socket is not acknowledged, and a take-over landing
    // between the upload's courtesy check and the frame reaching the server
    // makes the server drop it silently. So the toast says what dux did.
    for (const t of [
      dropToastFor([sent("a.png")], terminal),
      dropToastFor([sent("a.png"), sent("b.png")], terminal),
      dropToastFor([sent("a.png", "a-1.png")], terminal),
      dropToastFor(mixed, terminal),
      dropToastFor(
        [
          sent("a.png"),
          { kind: "refused", requestedName: "c.png", reason: "too big" },
        ],
        terminal,
      ),
    ]) {
      expect(t.message).not.toContain("pasted")
      expect(t.message).not.toContain("Pasted")
    }
    expect(dropToastFor([sent("a.png")], terminal).message).toContain(
      "sent its path",
    )
  })
})

describe("stranded files whose reasons differ", () => {
  // The uploads are sequential, so the reasons genuinely differ within one
  // drop: a reconnect strands the first file ("the connection dropped"), and
  // another device taking over strands a later one ("another device took over
  // input"). The rung took notSent[0].reason and applied it to all of them, so
  // the second file's reason was discarded and the message said the connection
  // had dropped when it had not. Recorded before the fix, for the two files
  // below: "Saved, but the path was not sent: the connection dropped. The file
  // is at a.png (/home/p/code/app/a.png), b.png (/home/p/code/app/b.png)."
  const dropped = "the connection dropped"
  const takeover = "another device took over input"

  it("names each reason, not just the first", () => {
    const t = dropToastFor(
      [
        notSent("a.png", "a.png", "~/code/app", dropped),
        notSent("b.png", "b.png", "~/code/app", takeover),
      ],
      terminal,
    )
    expect(t.tone).toBe("warning")
    expect(t.message).toContain(dropped)
    expect(t.message).toContain(takeover)
    // And each file is attached to the reason that actually applies to it.
    expect(t.message).toContain(
      "a.png (/home/p/code/app/a.png) because the connection dropped",
    )
    expect(t.message).toContain(
      "b.png (/home/p/code/app/b.png) because another device took over input",
    )
  })

  it("groups files that share a reason instead of repeating it", () => {
    const t = dropToastFor(
      [
        notSent("a.png", "a.png", "~/code/app", dropped),
        notSent("b.png", "b.png", "~/code/app", takeover),
        notSent("c.png", "c.png", "~/code/app", dropped),
      ],
      terminal,
    )
    expect(t.message).toContain(
      "a.png (/home/p/code/app/a.png), c.png (/home/p/code/app/c.png) because the connection dropped",
    )
    expect(t.message.match(/the connection dropped/g)).toHaveLength(1)
  })

  it("still reads plainly when every stranded file has the same reason", () => {
    // The common case must not be dressed up as a list of one.
    const t = dropToastFor(
      [
        notSent("a.png", "a.png", "~/code/app", dropped),
        notSent("b.png", "b.png", "~/code/app", dropped),
      ],
      terminal,
    )
    expect(t.message).toContain(`but the path was not sent: ${dropped}.`)
    expect(t.message).not.toContain("because")
  })

  it("keeps every reason even when one of them has more files than it will name", () => {
    // The overflow must not become the discard this fix exists to remove: a
    // reason with too many files to name still says how many more there are,
    // and still says its reason.
    const many = Array.from({ length: MAX_NAMED_FILES + 2 }, (_, i) =>
      notSent(`f${i}.png`, `f${i}.png`, "~/code/app", dropped),
    )
    const t = dropToastFor(
      [...many, notSent("z.png", "z.png", "~/code/app", takeover)],
      terminal,
    )
    expect(t.message).toContain(dropped)
    expect(t.message).toContain(takeover)
    expect(t.message).toContain("and 2 more")
    expect(t.message).toContain(
      "z.png (/home/p/code/app/z.png) because another device took over input",
    )
  })
})
