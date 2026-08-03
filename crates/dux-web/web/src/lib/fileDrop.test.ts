import { describe, expect, it } from "vitest"

import {
  MAX_NAMED_FILES,
  dragDropPasteFormFor,
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

/// The awkward inputs every form is measured against: a plain path, a space, an
/// apostrophe, a double quote, a dollar, a backtick and a backslash. One constant
/// so no form is quietly tested on an easier set than another.
const AWKWARD = {
  plain: "/home/p/shot.png",
  space: "/home/p/Web App/shot.png",
  apostrophe: "/home/p/Bob's app/shot.png",
  doubleQuote: '/home/p/it"s/shot.png',
  dollar: "/home/p/$(rm -rf ~)/shot.png",
  backtick: "/home/p/`whoami`/shot.png",
  backslash: "/home/p/a\\b/shot.png",
}

/// A hand-written POSIX shell lexer, just enough to answer the ONE question the
/// receiving CLI actually asks: does this text come out as exactly one word?
///
/// Written here rather than pulled in as a dependency, because the property under
/// test is small and a dependency would move the thing being tested out of the
/// repository. It implements the POSIX token-recognition rules a lexer uses (which
/// is all Codex's shlex does): unquoted whitespace separates words; a backslash
/// outside quotes escapes the next character; single quotes make everything
/// literal until the next single quote; inside double quotes a backslash escapes
/// only `"`, `\`, `$` and a backtick, and everything else, INCLUDING `$` and a
/// backtick on their own, is an ordinary character. It deliberately does NOT
/// expand anything: a lexer counts words, it does not run them.
///
/// Returns the tokens, so a test can assert both the count AND that the one token
/// is the original path rather than merely some single word.
function posixLex(input: string): string[] {
  const tokens: string[] = []
  let token = ""
  let started = false
  let i = 0
  while (i < input.length) {
    const c = input[i]
    if (/\s/.test(c)) {
      if (started) {
        tokens.push(token)
        token = ""
        started = false
      }
      i += 1
    } else if (c === "\\") {
      if (i + 1 >= input.length) throw new Error("trailing backslash")
      token += input[i + 1]
      started = true
      i += 2
    } else if (c === "'") {
      const close = input.indexOf("'", i + 1)
      if (close === -1) throw new Error("unterminated single quote")
      token += input.slice(i + 1, close)
      started = true
      i = close + 1
    } else if (c === '"') {
      i += 1
      started = true
      for (;;) {
        if (i >= input.length) throw new Error("unterminated double quote")
        if (input[i] === '"') {
          i += 1
          break
        }
        if (input[i] === "\\" && '"\\$`'.includes(input[i + 1] ?? "")) {
          token += input[i + 1]
          i += 2
          continue
        }
        token += input[i]
        i += 1
      }
    } else {
      token += c
      started = true
      i += 1
    }
  }
  if (started) tokens.push(token)
  return tokens
}

describe("posixLex (the check the quoting tests lean on)", () => {
  // A checker nobody checks proves nothing about the thing it checks, so pin its
  // behaviour on cases whose answer is not in doubt.
  it("splits on unquoted whitespace and keeps quoted runs whole", () => {
    expect(posixLex("a b")).toEqual(["a", "b"])
    expect(posixLex("'a b'")).toEqual(["a b"])
    expect(posixLex('"a b"')).toEqual(["a b"])
    expect(posixLex("a\\ b")).toEqual(["a b"])
  })

  it("treats a dollar and a backtick as ordinary characters, because it lexes", () => {
    expect(posixLex('"$x`y`"')).toEqual(["$x`y`"])
    expect(posixLex("'$x`y`'")).toEqual(["$x`y`"])
  })

  it("unescapes only the four characters a double quote gives meaning to", () => {
    expect(posixLex('"a\\"b"')).toEqual(['a"b'])
    expect(posixLex('"a\\\\b"')).toEqual(["a\\b"])
    expect(posixLex('"a\\nb"')).toEqual(["a\\nb"])
  })
})

describe("pastePayload", () => {
  /// Every form ends the same way, and that ending is load-bearing: a newline
  /// SUBMITS in these tools, so a file arriving with an automatic submit would
  /// fire a half-written prompt.
  function expectTrailer(payload: string) {
    expect(payload.endsWith(" ")).toBe(true)
    expect(payload).not.toContain("\n")
    expect(payload).not.toContain("\r")
  }

  describe("bare", () => {
    it("adds nothing at all, to anything", () => {
      for (const path of Object.values(AWKWARD)) {
        expect(pastePayload(path, "bare")).toBe(`${path} `)
      }
    })

    it("keeps the one trailing space and no newline", () => {
      expectTrailer(pastePayload(AWKWARD.space, "bare"))
    })
  })

  describe("single_quoted", () => {
    it("produces the exact expected string for each awkward input", () => {
      // Inside POSIX single quotes nothing is special, so the dollar, the
      // backtick, the double quote and the backslash all come out untouched.
      // Only the apostrophe is handled, by closing, escaping and reopening.
      expect(pastePayload(AWKWARD.plain, "single_quoted")).toBe(
        "'/home/p/shot.png' ",
      )
      expect(pastePayload(AWKWARD.space, "single_quoted")).toBe(
        "'/home/p/Web App/shot.png' ",
      )
      expect(pastePayload(AWKWARD.apostrophe, "single_quoted")).toBe(
        "'/home/p/Bob'\\''s app/shot.png' ",
      )
      expect(pastePayload(AWKWARD.doubleQuote, "single_quoted")).toBe(
        `'/home/p/it"s/shot.png' `,
      )
      expect(pastePayload(AWKWARD.dollar, "single_quoted")).toBe(
        "'/home/p/$(rm -rf ~)/shot.png' ",
      )
      expect(pastePayload(AWKWARD.backtick, "single_quoted")).toBe(
        "'/home/p/`whoami`/shot.png' ",
      )
      expect(pastePayload(AWKWARD.backslash, "single_quoted")).toBe(
        "'/home/p/a\\b/shot.png' ",
      )
    })

    it("lexes to exactly ONE token, which is the path, for every input", () => {
      // This is the property Codex actually requires: it accepts a pasted path
      // only when POSIX lexing yields a single token. Asserted on the payload
      // MINUS its trailing space, since that space is the separator dux adds
      // after the path and is not part of it.
      for (const path of Object.values(AWKWARD)) {
        const quoted = pastePayload(path, "single_quoted").slice(0, -1)
        expect(posixLex(quoted)).toEqual([path])
      }
    })

    it("keeps the one trailing space and no newline", () => {
      expectTrailer(pastePayload(AWKWARD.apostrophe, "single_quoted"))
    })
  })

  describe("double_quoted", () => {
    it("produces the exact expected string for each awkward input", () => {
      // Only the double quote and the backslash are escaped. A dollar and a
      // backtick are left alone deliberately: the receiving end is a LEXER
      // counting words, not an evaluator expanding them.
      expect(pastePayload(AWKWARD.plain, "double_quoted")).toBe(
        `"/home/p/shot.png" `,
      )
      expect(pastePayload(AWKWARD.space, "double_quoted")).toBe(
        `"/home/p/Web App/shot.png" `,
      )
      expect(pastePayload(AWKWARD.apostrophe, "double_quoted")).toBe(
        `"/home/p/Bob's app/shot.png" `,
      )
      expect(pastePayload(AWKWARD.doubleQuote, "double_quoted")).toBe(
        `"/home/p/it\\"s/shot.png" `,
      )
      expect(pastePayload(AWKWARD.dollar, "double_quoted")).toBe(
        `"/home/p/$(rm -rf ~)/shot.png" `,
      )
      expect(pastePayload(AWKWARD.backtick, "double_quoted")).toBe(
        '"/home/p/`whoami`/shot.png" ',
      )
      expect(pastePayload(AWKWARD.backslash, "double_quoted")).toBe(
        `"/home/p/a\\\\b/shot.png" `,
      )
    })

    it("lexes to exactly ONE token, which is the path, for every input", () => {
      for (const path of Object.values(AWKWARD)) {
        const quoted = pastePayload(path, "double_quoted").slice(0, -1)
        expect(posixLex(quoted)).toEqual([path])
      }
    })

    it("keeps the one trailing space and no newline", () => {
      expectTrailer(pastePayload(AWKWARD.doubleQuote, "double_quoted"))
    })
  })

  describe("backslash_escaped", () => {
    it("produces the exact expected string for each awkward input", () => {
      expect(pastePayload(AWKWARD.plain, "backslash_escaped")).toBe(
        "/home/p/shot.png ",
      )
      expect(pastePayload(AWKWARD.space, "backslash_escaped")).toBe(
        "/home/p/Web\\ App/shot.png ",
      )
      expect(pastePayload(AWKWARD.apostrophe, "backslash_escaped")).toBe(
        "/home/p/Bob\\'s\\ app/shot.png ",
      )
      expect(pastePayload(AWKWARD.doubleQuote, "backslash_escaped")).toBe(
        '/home/p/it\\"s/shot.png ',
      )
      expect(pastePayload(AWKWARD.dollar, "backslash_escaped")).toBe(
        "/home/p/\\$\\(rm\\ -rf\\ \\~\\)/shot.png ",
      )
      expect(pastePayload(AWKWARD.backtick, "backslash_escaped")).toBe(
        "/home/p/\\`whoami\\`/shot.png ",
      )
      expect(pastePayload(AWKWARD.backslash, "backslash_escaped")).toBe(
        "/home/p/a\\\\b/shot.png ",
      )
    })

    it("lexes to exactly ONE token, which is the path, for every input", () => {
      // Not required by the brief for this form, but it is the same property
      // and it is the reason Codex accepts this form at all.
      for (const path of Object.values(AWKWARD)) {
        const escaped = pastePayload(path, "backslash_escaped").slice(0, -1)
        expect(posixLex(escaped)).toEqual([path])
      }
    })

    it("leaves a non-ASCII path alone rather than escaping every codepoint", () => {
      // Over-escaping is a no-op to a lexer but not to a reader, and the users
      // most likely to have such a path are exactly the ones it would hurt.
      expect(pastePayload("/tmp/スクリーンショット.png", "backslash_escaped")).toBe(
        "/tmp/スクリーンショット.png ",
      )
    })

    it("keeps the one trailing space and no newline", () => {
      expectTrailer(pastePayload(AWKWARD.space, "backslash_escaped"))
    })
  })
})

describe("dragDropPasteFormFor", () => {
  const forms = { claude: "bare", codex: "single_quoted", opencode: "bare" }

  it("uses the form the server published for the running provider", () => {
    expect(dragDropPasteFormFor(forms, "codex")).toBe("single_quoted")
    expect(dragDropPasteFormFor(forms, "claude")).toBe("bare")
  })

  it("falls back to bare for a provider the server said nothing about", () => {
    // A provider the user added themselves, an older server that does not send
    // the map at all, and a pane with no provider (a plain terminal). Bare is
    // the do-nothing option.
    expect(dragDropPasteFormFor(forms, "myagent")).toBe("bare")
    expect(dragDropPasteFormFor(undefined, "codex")).toBe("bare")
    expect(dragDropPasteFormFor(forms, undefined)).toBe("bare")
  })

  it("falls back to bare for a form name it does not recognize", () => {
    // The server normalizes and warns once at config load, so this should not
    // arise; a client that trusted the string blindly would still be one config
    // typo from pasting the literal word into somebody's prompt.
    expect(dragDropPasteFormFor({ codex: "single-quoted" }, "codex")).toBe("bare")
    expect(dragDropPasteFormFor({ codex: "file_url" }, "codex")).toBe("bare")
  })

  it("recognizes all four shipped forms", () => {
    for (const form of [
      "bare",
      "single_quoted",
      "double_quoted",
      "backslash_escaped",
    ] as const) {
      expect(dragDropPasteFormFor({ p: form }, "p")).toBe(form)
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
