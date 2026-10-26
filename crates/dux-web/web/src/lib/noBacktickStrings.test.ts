import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// A toast never shows a literal backtick.
//
// The browser's toasts (raised through `lib/notify.ts`) draw their text as it
// is, so a command written between backticks shows the backticks. A name in a
// toast the browser raises itself is a `Prose` name, which `notify` draws as
// the `InlineCode` chip. The server's statuses carry their names the same way,
// and the Rust side has its own guard for them
// (`crates/dux-core/tests/status_messages_carry_no_backticks.rs`).
//
// This reads every production source file under src/ and fails on a quoted
// string ('…' or "…") holding a backtick, or an escaped backtick inside a
// template literal, that is not in ALLOWED with its reason. Most such strings
// are code, or copy rendered through `renderInlineCode`, which turns a
// backticked span into the chip.
//
// WHAT IT CANNOT SEE. It is a small tokenizer, not a TypeScript parser: it
// knows comments, quoted strings and template literals (with `${}` nesting),
// and nothing of regular-expression literals or JSX text, so an apostrophe in
// JSX text opens a "string" that it closes at the end of that line. A backtick
// built at runtime, or text that arrives from the server (an HTTP refusal, a
// relayed error), is invisible to it.

const srcDir = fileURLToPath(new URL("../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

function isProduction(name: string): boolean {
  return /\.tsx?$/.test(name) && !name.includes(".test.") && !name.endsWith(".d.ts")
}

// The trimmed lines holding a backtick inside a quoted string, or an escaped
// backtick inside a template literal.
function backtickLines(source: string): string[] {
  const lines = source.split("\n")
  const flagged = new Set<number>()
  let line = 0
  // One entry per open template literal: how many braces deep its current
  // `${}` is, or -1 while reading the template's own text.
  const templates: number[] = []
  let i = 0
  const inTemplateText = () => templates.length > 0 && templates[templates.length - 1] === -1
  while (i < source.length) {
    const c = source[i]
    const next = source[i + 1]
    if (c === "\n") line++
    if (inTemplateText()) {
      if (c === "\\") {
        if (next === "`") flagged.add(line)
        if (next === "\n") line++
        i += 2
      } else if (c === "`") {
        templates.pop()
        i++
      } else if (c === "$" && next === "{") {
        templates[templates.length - 1] = 0
        i += 2
      } else {
        i++
      }
      continue
    }
    if (c === "/" && next === "/") {
      while (i < source.length && source[i] !== "\n") i++
    } else if (c === "/" && next === "*") {
      const end = source.indexOf("*/", i + 2)
      const stop = end === -1 ? source.length : end + 2
      for (let j = i; j < stop; j++) if (source[j] === "\n") line++
      i = stop
    } else if (c === "'" || c === '"') {
      let j = i + 1
      while (j < source.length && source[j] !== c && source[j] !== "\n") {
        if (source[j] === "`") flagged.add(line)
        j += source[j] === "\\" && source[j + 1] !== "\n" ? 2 : 1
      }
      i = source[j] === c ? j + 1 : j
    } else if (c === "`") {
      templates.push(-1)
      i++
    } else {
      if (templates.length > 0) {
        const top = templates.length - 1
        if (c === "{") templates[top]++
        else if (c === "}") templates[top] = templates[top] === 0 ? -1 : templates[top] - 1
      }
      i++
    }
  }
  return [...flagged].sort((a, b) => a - b).map((n) => lines[n].trim())
}

// Deliberate exceptions, each with the reason it is not a toast's text. An
// entry that no longer matches anything fails the suite, so the list cannot rot.
const ALLOWED: { file: string; line: string; reason: string }[] = [
  {
    file: "components/CustomizeWebappDialog.tsx",
    line: '"Configure dux. Saved to `config.toml` and applied to every connected browser.",',
    reason: "The Settings dialog's description, drawn through renderInlineCode, which chips the span.",
  },
  {
    file: "components/Welcome.tsx",
    line: 'const parts = tip.split("`")',
    reason: "The welcome tip renderer, splitting on the delimiter it hides.",
  },
  {
    file: "lib/inlineMarkdown.tsx",
    line: 'const parts = text.split("`")',
    reason: "renderInlineCode itself, splitting on the delimiter it turns into chips.",
  },
  {
    file: "lib/inlineMarkdown.tsx",
    line: "const literal = hasDanglingBacktick && isLastPart ? `\\`${part}` : part",
    reason: "renderInlineCode putting back an unpaired backtick it did not consume.",
  },
  {
    file: "lib/termselect.ts",
    line: "export const DEFAULT_WORD_SEPARATORS = \" ()[]{}',\\\"`\"",
    reason: "The terminal's word separators: a backtick ends a word on a double-click.",
  },
  {
    file: "lib/fileDrop.ts",
    line: 'return `"${path.replaceAll(/[\\\\"$`]/g, (c) => `\\\\${c}`)}"`',
    reason: "Shell-quoting a dropped path: a backtick is one of the characters it escapes.",
  },
  {
    file: "lib/fileDrop.ts",
    line: "const SHELL_SIGNIFICANT = /[\\s\"#$&'()*;<>?[\\\\\\]`{|}~]/g",
    reason: "A regular expression of the characters a shell treats specially, a backtick among them.",
  },
  {
    file: "lib/authErrors.ts",
    line: '"No password is set. The first one can only be set from this machine, from your tailnet, or by running `dux config set server.auth.password` where dux runs."',
    reason: "The Preferences password row's note, drawn through renderInlineCode, which chips the span.",
  },
  {
    file: "lib/settingsDescriptors.ts",
    line: '"Asked of every browser that reaches dux from where `[server.auth] require` says it applies. dux keeps only an Argon2 hash in `config.toml`; anyone who can read that file can still try to guess the password offline, so use a long one you use nowhere else. Changing it signs every browser out, this one included. The first password can only be set from this machine, your tailnet, or with `dux config set server.auth.password`.",',
    reason: "A setting's description, drawn through renderInlineCode in the Settings dialog.",
  },
  {
    file: "lib/settingsDescriptors.ts",
    line: '"Syncs pull-request status for your agents in the background using the `gh` CLI, showing a PR pill on branches with an open, merged, or closed pull request. When off, dux stops polling and clears cached PR statuses. Requires `gh` to be installed and authenticated.",',
    reason: "A setting's description, drawn through renderInlineCode in the Settings dialog.",
  },
  {
    file: "lib/settingsDescriptors.ts",
    line: '? "This run of dux was started with `--no-tailscale`, so the Tailscale address stays unbound however this is set. Your choice is saved and used the next time dux starts without that flag."',
    reason: "A setting's lock note, drawn through renderInlineCode in the Settings dialog.",
  },
]

describe("no toast text is written with a backtick", () => {
  const hits = walk(srcDir)
    .map((path) => ({ name: relative(srcDir, path), path }))
    .filter(({ name }) => isProduction(name) && !name.startsWith("test/") && name !== "testSetup.ts")
    .flatMap(({ name, path }) =>
      backtickLines(readFileSync(path, "utf-8")).map((line) => ({ file: name, line })),
    )

  it("finds every backtick string in an allowed place", () => {
    const unexplained = hits.filter(
      (hit) => !ALLOWED.some((allowed) => allowed.file === hit.file && allowed.line === hit.line),
    )
    expect(unexplained, "a backtick in a string shows verbatim in a toast; pass the name as Prose").toEqual([])
  })

  it("keeps no allowance for a line that is gone", () => {
    const stale = ALLOWED.filter(
      (allowed) => !hits.some((hit) => hit.file === allowed.file && hit.line === allowed.line),
    )
    expect(stale).toEqual([])
  })

  it("tells strings from comments and template delimiters", () => {
    expect(backtickLines('notify.error("run `x`")\n')).toHaveLength(1)
    expect(backtickLines("notify.error('run `x`')\n")).toHaveLength(1)
    expect(backtickLines("const s = `a \\`b\\``\n")).toHaveLength(1)
    expect(backtickLines("// a `comment`\nconst s = `template ${x}`\n")).toEqual([])
    expect(backtickLines("/* `block`\n `comment` */\n")).toEqual([])
    expect(backtickLines('const s = `Macro "${name}" needs text.`\n')).toEqual([])
  })
})
