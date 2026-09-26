// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it, vi } from "vitest"
import { render } from "@testing-library/react"

import {
  branchWarningProse,
  HEURISTIC_BRANCH_NOTE_PROSE,
  worktreeBaseNoteProse,
} from "./addProjectWarning"
import { changeBaseBranchProse } from "./changeBaseBranch"
import { checkoutDefaultBranchProse } from "./checkoutDefaultBranch"
import { detachConfirmProse } from "./detachAgent"
import { deleteProjectProse, removeProjectProse } from "./projectConfirm"
import {
  chip,
  type Prose,
  type ProseSegment,
  proseText,
  quotedChip,
  renderProse,
  endProse,
  joinProse,
  prose,
  wireProse,
} from "./prose"
import { recreateConfirmProse } from "./recreateWorkingCopy"

describe("a name's bidi controls", () => {
  it("are dropped by both chip builders", () => {
    expect(chip("a\u202Eb\u2066c\u2069")).toEqual({ name: "abc", quoted: false })
    expect(quotedChip("\u200Fx\u061C")).toEqual({ name: "x", quoted: true })
  })

  it("never reach the recreate confirm's text", () => {
    const text = proseText(
      recreateConfirmProse("~/wt", "feat\u202Etxt.exe", "main", true, ["claude"]),
    )
    expect(text).toContain('"feattxt.exe"')
    expect(text).not.toMatch(/[\u202A-\u202E\u2066-\u2069\u200E\u200F\u061C]/)
  })
})

describe("a sentence built from prose and names", () => {
  it("renders every name as the shared chip and the rest as text", () => {
    const { container } = render(
      <p>{renderProse(["Delete ", chip("feat/x"), " from ", quotedChip("~/repo"), "?"])}</p>,
    )
    const codes = [...container.querySelectorAll("code")].map((c) => c.textContent)
    expect(codes).toEqual(["feat/x", "~/repo"])
    expect(container.querySelector("code")?.dataset.slot).toBe("inline-code")
    // No quotes on the web: the chip is the delimiter.
    expect(container.textContent).toBe("Delete feat/x from ~/repo?")
  })

  it("spells the plain-text form with quotes only where the name was quoted", () => {
    expect(
      proseText(["on branch ", chip("dev"), ", from ", quotedChip("main"), "."]),
    ).toBe('on branch dev, from "main".')
  })

  it("renders an empty name as an empty chip rather than dropping it", () => {
    const { container } = render(<p>{renderProse(["a ", chip(""), " b"])}</p>)
    expect(container.querySelectorAll("code")).toHaveLength(1)
  })
})

describe("the sentences both surfaces print", () => {
  // The other half of the pin in dux-core's `prose.rs`: both read this same
  // file, so a sentence passes only when the terminal UI and the web say the
  // same words and mark the same names. Adjacent strings are merged first, so
  // either side may split its constant words however it likes.
  const fixture = JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "../../../../dux-core/tests/fixtures/prose_cross_language.json",
      ),
      "utf8",
    ),
  ) as {
    cases: {
      what: string
      sentence: string
      args: Record<string, unknown>
      segments: Prose
    }[]
  }

  function merged(prose: Prose): Prose {
    const out: ProseSegment[] = []
    for (const segment of prose) {
      const last = out[out.length - 1]
      if (typeof segment === "string" && typeof last === "string") {
        out[out.length - 1] = last + segment
      } else {
        out.push(segment)
      }
    }
    return out
  }

  function build(sentence: string, args: Record<string, unknown>): Prose {
    switch (sentence) {
      case "detach_confirm":
        return detachConfirmProse(
          args.label as string,
          args.grace_seconds as number,
          args.live_tabs as number,
        )
      case "recreate_confirm":
        return recreateConfirmProse(
          args.worktree_label as string,
          args.branch_name as string,
          args.source_branch as string,
          args.conversation_resumes as boolean,
          args.providers as string[],
        )
      case "checkout_default_branch_confirm":
        return checkoutDefaultBranchProse(
          args.project_name as string,
          args.stored_base as string | null,
        )
      case "change_base_branch_confirm":
        return changeBaseBranchProse(
          args.project_name as string,
          args.from as string | null,
          args.to as string,
        )
      case "add_project_branch_warning":
        return branchWarningProse(
          args.current_branch as string,
          args.default_branch as string | null,
        )
      case "add_project_worktree_base":
        return worktreeBaseNoteProse(args.branch as string)
      case "add_project_heuristic_note":
        return HEURISTIC_BRANCH_NOTE_PROSE
      case "delete_project_confirm":
        return deleteProjectProse(
          (args.project_name as string | null) ?? undefined,
          args.agent_count as number,
        )
      case "remove_project_confirm":
        return removeProjectProse(
          (args.project_name as string | null) ?? undefined,
          args.agent_count as number,
        )
      default:
        throw new Error(`the fixture names a sentence this test cannot build: ${sentence}`)
    }
  }

  it("has not lost its cases", () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(10)
  })

  it.each(fixture.cases.map((c) => [c.what, c] as const))(
    "agrees with the terminal UI about %s",
    (_what, c) => {
      expect(merged(build(c.sentence, c.args))).toEqual(merged(c.segments))
    },
  )
})

describe("building a sentence in the browser", () => {
  it("turns a template's names into chips and everything else into words", () => {
    const built = prose`Saved ${chip("a b.txt")} to ${chip("~/up")} (${3} files).`
    expect(built).toEqual([
      "Saved ",
      chip("a b.txt"),
      " to ",
      chip("~/up"),
      " (3 files).",
    ])
  })

  it("splices a sentence built elsewhere, merging the words at the seams", () => {
    const inner = prose`${chip("x")} and more`
    expect(prose`Got ${inner}.`).toEqual(["Got ", chip("x"), " and more."])
  })

  it("joins sentences with a separator", () => {
    expect(joinProse([[chip("a")], [chip("b")], ["c"]], ", ")).toEqual([
      chip("a"),
      ", ",
      chip("b"),
      ", c",
    ])
    expect(joinProse([], ", ")).toEqual([])
  })

  it("ends a sentence with exactly one terminator, whatever it ends in", () => {
    expect(proseText(endProse(prose`Could not save ${chip("f")}`))).toBe(
      "Could not save f.",
    )
    expect(proseText(endProse(prose`Done.  `))).toBe("Done.")
    expect(proseText(endProse(prose`Ask ${chip("why?")}`))).toBe("Ask why?")
    expect(endProse([])).toEqual([])
  })
})

describe("a status sentence read off the wire", () => {
  const message = 'Checked out "main" in /src/app.'
  const segments = [
    "Checked out ",
    { name: "main", quoted: true },
    " in ",
    { name: "/src/app", quoted: false },
    ".",
  ]

  it("takes the parts when they spell the message exactly", () => {
    expect(wireProse(message, segments)).toEqual(segments)
  })

  it("keeps the plain message when the server sent no parts", () => {
    expect(wireProse(message, undefined)).toBe(message)
    expect(wireProse(message, null)).toBe(message)
  })

  it("keeps the plain message when the parts are malformed", () => {
    expect(wireProse(message, "Checked out")).toBe(message)
    expect(wireProse(message, [{ name: 3, quoted: true }])).toBe(message)
    expect(wireProse(message, [{ name: "main" }])).toBe(message)
    expect(wireProse(message, [7])).toBe(message)
  })

  // A chip is drawn as one unit inside dux's own words; an override inside a
  // name (a branch from somebody else's pull request) would reorder them. An
  // older server may still send one, so the wire path strips it too, from the
  // parts and the plain fallback alike.
  it("strips bidi controls from a name that arrives off the wire", () => {
    const crafted = "feat\u202Etxt.exe"
    const got = wireProse(`Deleted "${crafted}".`, [
      "Deleted ",
      { name: crafted, quoted: true },
      ".",
    ])
    expect(got).toEqual(["Deleted ", { name: "feattxt.exe", quoted: true }, "."])
    expect(wireProse(`Deleted "${crafted}".`, undefined)).toBe('Deleted "feattxt.exe".')
  })

  it("keeps the plain message when the parts spell a different sentence", () => {
    // The message is the terminal UI's words; parts that disagree with it must
    // never put different words on the web.
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    try {
      expect(
        wireProse(message, ["Checked out ", { name: "dev", quoted: true }, " in /src/app."]),
      ).toBe(message)
      // The fallback hides a producer upstream that is wrong; the console is
      // where a developer can still find it.
      expect(warn).toHaveBeenCalledTimes(1)
    } finally {
      warn.mockRestore()
    }
  })
})
