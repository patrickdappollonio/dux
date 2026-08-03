import { describe, expect, it } from "vitest"

import {
  MAX_NAMED_FILES,
  TERMINAL_PASTE_FORM,
  attachmentCharLimitFor,
  dragDropPasteFor,
  dragDropPasteFormFor,
  dropToastFor,
  pasteExceedsAttachmentLimit,
  pastePayload,
  type ConfiguredDropPaste,
  type DragDropPasteForm,
  type DropOutcome,
  type DropPasteProfile,
} from "./fileDrop"

const ALL_FORMS: readonly DragDropPasteForm[] = [
  "bare",
  "single_quoted",
  "double_quoted",
  "backslash_escaped",
]

/// What the server publishes for CONFIG, keyed by provider name, written the
/// short way: a form name, or a `[form, command]` pair when the test cares which
/// CLI the block runs.
function published(
  byProvider: Record<string, string | [string, string]> | undefined,
): ConfiguredDropPaste {
  if (byProvider === undefined) return undefined
  return Object.fromEntries(
    Object.entries(byProvider).map(([name, v]) =>
      typeof v === "string"
        ? [name, { form: v, command_name: name }]
        : [name, { form: v[0], command_name: v[1] }],
    ),
  )
}

/// What a LIVE tab launched with, as the spine carries it on the tab.
function launched(form: string, command_name = "codex"): DropPasteProfile {
  return { form, command_name }
}

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

/// The word separators the real lexer uses, and no others. Exactly three
/// characters: a space, a tab and a newline. Notably NOT a carriage return, a
/// vertical tab, a form feed or a non-breaking space, all of which are ordinary
/// characters that live INSIDE a word.
const WORD_SEPARATORS = " \t\n"

/// A hand-written POSIX shell lexer, just enough to answer the ONE question the
/// receiving CLI actually asks: does this text come out as exactly one word?
///
/// Written here rather than pulled in as a dependency, because the property under
/// test is small and a dependency would move the thing being tested out of the
/// repository. What it is a model OF is not "POSIX in general" but one specific
/// program: `shlex` 1.3.0's `Shlex` iterator (`src/bytes.rs`), which is what Codex
/// pins and therefore the only lexer whose answer matters here. It is written to
/// mirror that source arm for arm, and the golden cases below pin the four places
/// an earlier, looser version of this function DISAGREED with it:
///
///   - a backslash-newline pair is REMOVED, inside double quotes and out; the
///     earlier version kept the newline, which would have reported a token that
///     the real lexer never produces;
///   - `#` starts a comment, but only where a word would start; the earlier
///     version had no notion of comments at all and lexed `#x` as a token;
///   - the separators are exactly space, tab and newline; the earlier version used
///     JavaScript's `\s`, which also splits on a carriage return, a form feed, a
///     vertical tab and every Unicode space, none of which the real lexer splits
///     on;
///   - a trailing backslash and an unclosed quote are ERRORS (`had_error`, which
///     makes `shlex::split` return `None`), which this models by throwing.
///
/// None of those four could change an answer for the paths dux actually sends,
/// because dux refuses a newline or a tab in a path. They are fixed anyway: the
/// value of this function is entirely in being a faithful model, and a checker
/// nobody checks proves nothing about the thing it checks.
///
/// One deliberate divergence: `shlex::bytes::Shlex` walks BYTES, and this walks
/// Unicode code points. Every character it gives meaning to is ASCII, so the two
/// agree on any valid UTF-8 input, and code points are what a JavaScript test can
/// honestly assert on.
///
/// Returns the tokens, so a test can assert both the count AND that the one token
/// is the original path rather than merely some single word.
function posixLex(input: string): string[] {
  const chars = [...input]
  let i = 0
  const next = (): string | undefined => (i < chars.length ? chars[i++] : undefined)

  // `Shlex::parse_single`: everything is literal until the closing quote.
  function parseSingle(): string {
    let out = ""
    for (;;) {
      const c = next()
      if (c === undefined) throw new Error("input ended inside a single-quoted string")
      if (c === "'") return out
      out += c
    }
  }

  // `Shlex::parse_double`: a backslash gives meaning to `"`, `\`, `$` and a
  // backtick, ERASES a following newline, and in front of anything else is kept
  // along with the character it failed to escape.
  function parseDouble(): string {
    let out = ""
    for (;;) {
      const c = next()
      if (c === undefined) throw new Error("input ended inside a double-quoted string")
      if (c === '"') return out
      if (c !== "\\") {
        out += c
        continue
      }
      const escaped = next()
      if (escaped === undefined) {
        throw new Error("input ended inside a double-quoted string")
      }
      if ('"\\$`'.includes(escaped)) out += escaped
      else if (escaped !== "\n") out += `\\${escaped}`
    }
  }

  // `Shlex::parse_word`.
  function parseWord(first: string): string {
    let ch: string | undefined = first
    let out = ""
    for (;;) {
      if (ch === '"') out += parseDouble()
      else if (ch === "'") out += parseSingle()
      else if (ch === "\\") {
        const escaped = next()
        if (escaped === undefined) {
          throw new Error("input ended right after an unescaped backslash")
        }
        // A backslash-newline pair is a line continuation and contributes
        // NOTHING, which is the case the earlier version of this function got
        // wrong: it preserved whatever followed the backslash, always.
        if (escaped !== "\n") out += escaped
      } else if (WORD_SEPARATORS.includes(ch)) break
      else out += ch
      ch = next()
      if (ch === undefined) break
    }
    return out
  }

  const tokens: string[] = []
  // `<Shlex as Iterator>::next`, called until it stops yielding.
  for (;;) {
    let ch = next()
    if (ch === undefined) return tokens
    // Skip leading separators and any whole comment. A `#` is a comment opener
    // ONLY here, at the position a word would start; inside a word it is an
    // ordinary character (there is no `#` arm in `parse_word`).
    for (;;) {
      if (WORD_SEPARATORS.includes(ch)) {
        // Nothing to do; fall through to reading the next character.
      } else if (ch === "#") {
        for (;;) {
          const c = next()
          if (c === undefined || c === "\n") break
        }
      } else break
      const c = next()
      if (c === undefined) return tokens
      ch = c
    }
    tokens.push(parseWord(ch))
  }
}

describe("posixLex (the check the quoting tests lean on)", () => {
  // A checker nobody checks proves nothing about the thing it checks, so pin its
  // behaviour on cases whose answer is not in doubt. Every expectation below was
  // read off `shlex` 1.3.0's `src/bytes.rs`, the lexer Codex pins.
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
    expect(posixLex('"a\\$b"')).toEqual(["a$b"])
    expect(posixLex('"a\\`b"')).toEqual(["a`b"])
    // Anything else keeps BOTH the backslash and the character after it.
    expect(posixLex('"a\\nb"')).toEqual(["a\\nb"])
  })

  it("starts a comment at a `#` that begins a word, and nowhere else", () => {
    expect(posixLex("#x")).toEqual([])
    expect(posixLex("a #b")).toEqual(["a"])
    expect(posixLex("#c\na")).toEqual(["a"])
    // Inside a word it is an ordinary character, and quoting it makes it one
    // even at the start.
    expect(posixLex("a#b")).toEqual(["a#b"])
    expect(posixLex("'#x'")).toEqual(["#x"])
  })

  it("removes a backslash-newline pair entirely, quoted or not", () => {
    expect(posixLex("a\\\nb")).toEqual(["ab"])
    expect(posixLex('"a\\\nb"')).toEqual(["ab"])
  })

  it("separates on a space, a tab and a newline, and on nothing else", () => {
    expect(posixLex("a\tb\nc d")).toEqual(["a", "b", "c", "d"])
    // A carriage return, a vertical tab, a form feed and a non-breaking space
    // are ordinary characters. JavaScript's `\s` matches all four, which is why
    // the earlier version of this lexer split on them.
    expect(posixLex("a\rb")).toEqual(["a\rb"])
    expect(posixLex("a\u000bb")).toEqual(["a\u000bb"])
    expect(posixLex("a\fb")).toEqual(["a\fb"])
    expect(posixLex("a\u00a0b")).toEqual(["a\u00a0b"])
  })

  it("errors rather than guessing when the input ends mid-token", () => {
    // The real lexer sets `had_error`, throws out the last token and stops,
    // which makes `shlex::split` return `None`. Throwing is how that is modelled
    // here: an answer would be a fiction.
    expect(() => posixLex("a\\")).toThrow()
    expect(() => posixLex("'a")).toThrow()
    expect(() => posixLex('"a')).toThrow()
    expect(() => posixLex('"a\\')).toThrow()
  })

  it("yields an empty token for an empty quoted string", () => {
    expect(posixLex("''")).toEqual([""])
    expect(posixLex('""')).toEqual([""])
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
      // All FOUR characters a double quote gives meaning to are escaped: the
      // quote, the backslash, the dollar and the backtick. The dollar and the
      // backtick used to be left alone, on the stated reasoning that escaping
      // them would change the bytes the CLI finally sees. That reasoning was
      // WRONG, and the case below is the proof: shell lexing turns `\$` back
      // into `$` and the backslash-backtick pair back into a backtick, so the
      // escape is lossless and the form is safe even in front of something that
      // EVALUATES rather than lexes.
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
        `"/home/p/\\$(rm -rf ~)/shot.png" `,
      )
      expect(pastePayload(AWKWARD.backtick, "double_quoted")).toBe(
        '"/home/p/\\`whoami\\`/shot.png" ',
      )
      expect(pastePayload(AWKWARD.backslash, "double_quoted")).toBe(
        `"/home/p/a\\\\b/shot.png" `,
      )
    })

    it("escapes the dollar and the backtick LOSSLESSLY, which is why it may", () => {
      // The whole correction in one assertion: the escaped payload lexes back to
      // the byte-for-byte original path. Nothing is added to what the CLI reads,
      // so there was never a cost to weigh against the safety.
      for (const path of [AWKWARD.dollar, AWKWARD.backtick]) {
        const quoted = pastePayload(path, "double_quoted").slice(0, -1)
        expect(quoted).toContain("\\")
        expect(posixLex(quoted)).toEqual([path])
      }
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
  const forms = published({
    claude: "bare",
    codex: "single_quoted",
    opencode: "bare",
  })
  const running = (provider: string | undefined) =>
    ({ kind: "agent", launched: undefined, provider }) as const

  it("uses the form the server published for the running provider", () => {
    expect(dragDropPasteFormFor(forms, running("codex"))).toBe("single_quoted")
    expect(dragDropPasteFormFor(forms, running("claude"))).toBe("bare")
  })

  it("prefers the form THIS TAB launched with over the provider's current one", () => {
    // The scenario a provider-keyed answer cannot serve. Two live tabs of one
    // provider, launched either side of a config edit: they report the same
    // provider name and need different forms. Each tab carries what it launched
    // with on the spine, and the pane resolves from its own tab.
    //
    // This is also why the launched form WINS rather than merely filling a gap.
    // If the current config value won, both tabs would resolve to it again and
    // publishing per tab would buy nothing. A config edit takes effect on the
    // tab's next launch.
    const config = published({ codex: "backslash_escaped" })
    expect(
      dragDropPasteFormFor(config, {
        kind: "agent",
        launched: launched("single_quoted"),
        provider: "codex",
      }),
    ).toBe("single_quoted")
    expect(
      dragDropPasteFormFor(config, {
        kind: "agent",
        launched: launched("backslash_escaped"),
        provider: "codex",
      }),
    ).toBe("backslash_escaped")
  })

  it("answers for a tab whose provider config no longer names", () => {
    // The user renamed or deleted `[providers.codex-nightly]` while the tab was
    // still running it. The provider map cannot answer and `bare` is the form
    // codex silently ignores, so the launched entry is the only truth left.
    expect(
      dragDropPasteFormFor(published({}), {
        kind: "agent",
        launched: launched("single_quoted"),
        provider: "codex-nightly",
      }),
    ).toBe("single_quoted")
  })

  it("falls back to the provider map for a tab with no live process", () => {
    // A dormant tab, or one whose launch has not reached this client yet. It
    // will launch with whatever config says now, so that is the right answer.
    expect(
      dragDropPasteFormFor(published({ codex: "single_quoted" }), {
        kind: "agent",
        launched: undefined,
        provider: "codex",
      }),
    ).toBe("single_quoted")
  })

  it("falls back to bare for a launched form name it does not recognize", () => {
    expect(
      dragDropPasteFormFor(published({ codex: "single_quoted" }), {
        kind: "agent",
        launched: launched("nonsense"),
        provider: "codex",
      }),
    ).toBe("bare")
  })

  it("falls back to bare for a provider the server said nothing about", () => {
    // A provider the user added themselves, an older server that does not send
    // the map at all, and a tab whose provider is not known yet. Bare is the
    // do-nothing option.
    expect(dragDropPasteFormFor(forms, running("myagent"))).toBe("bare")
    expect(dragDropPasteFormFor(published(undefined), running("codex"))).toBe(
      "bare",
    )
    expect(dragDropPasteFormFor(forms, running(undefined))).toBe("bare")
  })

  it("falls back to bare for a form name it does not recognize", () => {
    // The server normalizes and warns once at config load, so this should not
    // arise; a client that trusted the string blindly would still be one config
    // typo from pasting the literal word into somebody's prompt.
    expect(
      dragDropPasteFormFor(published({ codex: "single-quoted" }), running("codex")),
    ).toBe("bare")
    expect(
      dragDropPasteFormFor(published({ codex: "file_url" }), running("codex")),
    ).toBe("bare")
  })

  it("recognizes all four shipped forms", () => {
    for (const form of ALL_FORMS) {
      expect(dragDropPasteFormFor(published({ p: form }), running("p"))).toBe(form)
    }
  })

  it("gives a TERMINAL the shell-safe form, and reads no provider at all", () => {
    // The correction. A terminal runs a SHELL, and that is precisely why its
    // path has to be quoted rather than left bare: dux permits a dollar, a
    // backtick, a space, a semicolon, a quote and parentheses in a destination
    // path, and a shell splits and substitutes every one of them the moment the
    // user presses Enter on the line the path was pasted into.
    expect(TERMINAL_PASTE_FORM).toBe("single_quoted")
    expect(dragDropPasteFormFor(forms, { kind: "terminal" })).toBe(
      "single_quoted",
    )
    // The target type gives a terminal no provider FIELD and no launched
    // profile, so no configuration of either can change this answer.
    expect(dragDropPasteFormFor(published({}), { kind: "terminal" })).toBe(
      "single_quoted",
    )
    expect(dragDropPasteFormFor(published(undefined), { kind: "terminal" })).toBe(
      "single_quoted",
    )
  })

  it("makes a shell-hostile path inert for a terminal", () => {
    // What "shell-safe" has to mean, asserted on the bytes rather than on the
    // form name: the payload lexes to exactly ONE word, and that word is the
    // path, so nothing in it is a command, a variable or an argument boundary.
    for (const path of [
      "/home/p/$(rm -rf ~)/shot.png",
      "/home/p/`whoami`/shot.png",
      "/home/p/Web App/shot.png",
      "/home/p/a;b/shot.png",
      "/home/p/Bob's app/shot.png",
      '/home/p/it"s/shot.png',
    ]) {
      const form = dragDropPasteFormFor(published(undefined), {
        kind: "terminal",
      })
      expect(posixLex(pastePayload(path, form).slice(0, -1))).toEqual([path])
    }
  })
})

describe("the attachment length limit", () => {
  // Measured from the receiving CLI's own source rather than assumed: codex's
  // `chat_composer.rs` compares `char_count > LARGE_PASTE_CHAR_THRESHOLD` (1000)
  // and, when it is over, files the paste away as generic large content BEFORE
  // it ever looks for an image path. So a long enough path is never attached,
  // and quoting, which adds characters, is what can push one over.

  const codexTab = {
    kind: "agent",
    launched: launched("single_quoted", "codex"),
    provider: "codex",
  } as const

  it("belongs to the CLI, so it follows codex onto every form", () => {
    // It used to be keyed by FORM, which was wrong in both directions, and both
    // are pinned here. Codex configured with any of the other three forms
    // escaped the limit entirely and dux would send an over-limit payload codex
    // silently ignores.
    for (const form of ALL_FORMS) {
      const plan = dragDropPasteFor(published({ codex: form }), {
        kind: "agent",
        launched: undefined,
        provider: "codex",
      })
      expect(plan.form).toBe(form)
      expect(plan.charLimit).toBe(1000)
    }
  })

  it("follows the COMMAND, not the block name, in both directions", () => {
    // A provider's name is free text the user chooses, and the command it runs
    // is independent of it. Keyed by the NAME (which it was) the limit answered
    // for the wrong CLI both ways round: a real Codex under an alias got no
    // limit and was handed oversized paths it silently ignores, and an
    // unrelated CLI merely NAMED codex had valid long paths withheld from it.
    //
    // `[providers.myagent] command = "codex"` IS codex.
    expect(
      attachmentCharLimitFor(published({ myagent: ["bare", "codex"] }), {
        kind: "agent",
        launched: undefined,
        provider: "myagent",
      }),
    ).toBe(1000)
    // `[providers.codex] command = "something-else"` is NOT.
    expect(
      attachmentCharLimitFor(
        published({ codex: ["single_quoted", "something-else"] }),
        { kind: "agent", launched: undefined, provider: "codex" },
      ),
    ).toBe(null)
    // And the same holds for what a LIVE tab launched with, which is the
    // answer that actually applies to a running process.
    expect(
      attachmentCharLimitFor(published({}), {
        kind: "agent",
        launched: launched("bare", "codex"),
        provider: "myagent",
      }),
    ).toBe(1000)
    expect(
      attachmentCharLimitFor(published({}), {
        kind: "agent",
        launched: launched("single_quoted", "something-else"),
        provider: "codex",
      }),
    ).toBe(null)
  })

  it("identifies the CLI by its command's FILE NAME, so a full path works", () => {
    // `command` may be an absolute path, a relative one, or a bare name found
    // on PATH, and all three name the same CLI. The server compares on the file
    // name and publishes that, so this holds whichever way it is configured.
    for (const command of ["codex", "./codex", "/usr/local/bin/codex"]) {
      expect(
        attachmentCharLimitFor(published({ p: ["bare", command] }), {
          kind: "agent",
          launched: undefined,
          provider: "p",
        }),
        command,
      ).toBe(command === "codex" ? 1000 : null)
    }
  })

  it("gives a TERMINAL none, because a shell has none", () => {
    // The other direction of the same mistake. A terminal always uses the
    // shell-safe form, so keying by form made it inherit codex's composer limit,
    // and dux withheld a perfectly good path from a shell while telling the user
    // it was too long for "this agent" — which is not what it was talking to.
    const plan = dragDropPasteFor(published({ codex: "single_quoted" }), {
      kind: "terminal",
    })
    expect(plan.form).toBe("single_quoted")
    expect(plan.charLimit).toBe(null)
    const huge = pastePayload(`/tmp/${"a".repeat(5_000)}`, plan.form)
    expect(pasteExceedsAttachmentLimit(huge, plan.charLimit)).toBe(false)
  })

  it("gives a CLI with no measured limit none", () => {
    // Only codex has been measured. A CLI dux ships that has not, and one the
    // user added themselves, both get no limit: guessing one would withhold
    // files a CLI would have taken.
    for (const provider of ["claude", "opencode", "copilot", "myagent"]) {
      expect(
        attachmentCharLimitFor(published({ [provider]: "bare" }), {
          kind: "agent",
          launched: undefined,
          provider,
        }),
      ).toBe(null)
    }
    // ...as does a tab with no live process whose provider the server said
    // nothing about, and one whose provider is not known yet.
    expect(
      attachmentCharLimitFor(published({}), {
        kind: "agent",
        launched: undefined,
        provider: "codex",
      }),
    ).toBe(null)
    expect(
      attachmentCharLimitFor(published({ codex: "single_quoted" }), {
        kind: "agent",
        launched: undefined,
        provider: undefined,
      }),
    ).toBe(null)
    expect(attachmentCharLimitFor(published({}), codexTab)).toBe(1000)
  })

  it("measures the FINAL payload, not the path on disk", () => {
    // A path of exactly 998 characters is comfortably under the limit. Its
    // single-quoted payload is 1001, because two quotes and the trailing space
    // are part of what gets pasted, and that is over.
    const path = `/tmp/${"a".repeat(993)}`
    expect([...path].length).toBe(998)
    const payload = pastePayload(path, "single_quoted")
    expect([...payload].length).toBe(1001)
    expect(pasteExceedsAttachmentLimit(payload, 1000)).toBe(true)
    // The same path bare is 999 characters and fits, which is the whole point:
    // the answer belongs to the payload and not to the file. (Same CLI, same
    // limit: only the quoting differs, and that is what pushes it over.)
    expect([...pastePayload(path, "bare")].length).toBe(999)
    expect(pasteExceedsAttachmentLimit(pastePayload(path, "bare"), 1000)).toBe(
      false,
    )
  })

  it("puts the boundary exactly where the CLI puts it", () => {
    // `char_count > 1000`, so 1000 fits and 1001 does not.
    expect(pasteExceedsAttachmentLimit("x".repeat(1000), 1000)).toBe(false)
    expect(pasteExceedsAttachmentLimit("x".repeat(1001), 1000)).toBe(true)
  })

  it("counts characters, not UTF-16 code units", () => {
    // An emoji is one character to the CLI and two units to JavaScript. Counting
    // units would refuse a payload the CLI would have accepted.
    const payload = "🙂".repeat(600)
    expect(payload.length).toBe(1200)
    expect([...payload].length).toBe(600)
    expect(pasteExceedsAttachmentLimit(payload, 1000)).toBe(false)
  })

  it("never refuses anything when there is no limit", () => {
    expect(pasteExceedsAttachmentLimit("x".repeat(100_000), null)).toBe(false)
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
