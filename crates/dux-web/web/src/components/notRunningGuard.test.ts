import { readdirSync, readFileSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"


// A card saying an agent or one of its tabs is not running is the shared
// `NotRunningCard`, which owns its layout, the resume docs link and the Start
// button's touch size. This guard reads every source file under `src` (all
// folders, .ts and .tsx, tests excluded) and fails on the three marks a
// hand-built one leaves:
//
// - the resume docs link (`DOCS_AGENT_TABS_RESUME`, its `#how-resume-works`
//   address or its "How resume works" words);
// - the Start button a not-running surface draws (`<StartAgentButton`);
// - the sentence that says a part is not running ("isn't running", spelled
//   with a straight or curly apostrophe or as an entity).
//
// It reads text, not the program. A card built without any of the three (its own
// link, its own button, other words) is invisible to it, and a file that is
// allowed for one reason is not checked for the others it does not name. A
// caller that hands the shared card its own title says the sentence, so each
// such caller is allowed by exact occurrence below.

function sourcesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) return e.name === "node_modules" ? [] : sourcesUnder(p)
    return /\.tsx?$/.test(e.name) && !/\.test\.|\.d\.ts$/.test(e.name) ? [p] : []
  })
}

const root = fileURLToPath(new URL("../", import.meta.url))

type Mark = "resume-link" | "start-button" | "not-running-sentence"

const MARKS: Record<Mark, RegExp> = {
  "resume-link": /DOCS_AGENT_TABS_RESUME|how-resume-works|How resume works/g,
  "start-button": /<StartAgentButton\b/g,
  "not-running-sentence": /isn(?:'|’|&rsquo;|&#8217;|&#39;)t running/g,
}

interface Finding {
  file: string
  mark: Mark
  line: string
}

function findHandBuiltCards(files: Record<string, string>): Finding[] {
  const out: Finding[] = []
  for (const [file, source] of Object.entries(files)) {
    for (const line of source.split("\n")) {
      for (const mark of Object.keys(MARKS) as Mark[]) {
        MARKS[mark].lastIndex = 0
        if (MARKS[mark].test(line)) out.push({ file, mark, line: line.trim() })
      }
    }
  }
  return out
}

// One entry per exact occurrence: the file, the mark and a piece of the line,
// each occurrence one entry. An entry that matches nothing fails the suite.
const ALLOWED: { file: string; mark: Mark; contains: string; reason: string }[] = [
  { file: "components/NotRunningCard.tsx", mark: "resume-link", contains: "DOCS_AGENT_TABS_RESUME }", reason: "the card's import of the docs address" },
  { file: "components/NotRunningCard.tsx", mark: "resume-link", contains: "href={DOCS_AGENT_TABS_RESUME}", reason: "the card's own link" },
  { file: "components/NotRunningCard.tsx", mark: "resume-link", contains: "How resume works", reason: "the card's own link text" },
  { file: "components/NotRunningCard.tsx", mark: "start-button", contains: "<StartAgentButton", reason: "the card's Start button" },
  { file: "lib/docs.ts", mark: "resume-link", contains: "DOCS_AGENT_TABS_RESUME =", reason: "the address's definition, on the line with its #how-resume-works anchor" },
  { file: "lib/docs.ts", mark: "resume-link", contains: "How resume works", reason: "the comment naming the docs section" },
  { file: "components/StoppedAgentParts.tsx", mark: "start-button", contains: "<StartAgentButton", reason: "the thin band above the dock: one line, not a card, deliberately sharing only the Start button and the run output" },
  { file: "components/StoppedAgentParts.tsx", mark: "not-running-sentence", contains: "isn&rsquo;t running.", reason: "the band's own one-line sentence" },
  { file: "components/StoppedAgentParts.tsx", mark: "not-running-sentence", contains: "isn&rsquo;t running.", reason: "the sentence of the title it hands NotRunningCard" },
  { file: "components/DormantTabCard.tsx", mark: "not-running-sentence", contains: "isn&rsquo;t running.", reason: "the sentence of the title it hands NotRunningCard" },
]

function unallowed(findings: Finding[]): { rest: Finding[]; unused: string[] } {
  const used = new Set<number>()
  const rest = findings.filter((f) => {
    const i = ALLOWED.findIndex(
      (a, k) =>
        !used.has(k) && a.file === f.file && a.mark === f.mark && f.line.includes(a.contains),
    )
    if (i < 0) return true
    used.add(i)
    return false
  })
  const unused = ALLOWED.filter((_, i) => !used.has(i)).map((a) => `${a.file}: ${a.contains}`)
  return { rest, unused }
}

describe("a not-running card is the shared NotRunningCard", () => {
  it("finds no hand-built one in any folder under src", () => {
    const files = Object.fromEntries(
      sourcesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
    )
    const { rest, unused } = unallowed(findHandBuiltCards(files))
    expect(
      rest.map((f) => `${f.file} (${f.mark}): ${f.line}`),
      "build it with <NotRunningCard>, or allow the exact occurrence with a reason",
    ).toEqual([])
    expect(unused, "an allowed occurrence that no longer exists: remove its entry").toEqual([])
  })

  it("sees each mark, wherever it sits", () => {
    const cases: Record<string, string> = {
      "a/one.tsx": `<a href={DOCS_AGENT_TABS_RESUME}>x</a>`,
      "a/b/two.tsx": `<StartAgentButton sessionId={id} onStart={go}>Start</StartAgentButton>`,
      "three.tsx": `<p>This tab isn’t running.</p>`,
      "four.tsx": `<p>The tab isn't running.</p>`,
    }
    const hit = new Set(findHandBuiltCards(cases).map((f) => f.file))
    expect([...hit].sort()).toEqual(Object.keys(cases).sort())
  })
})
