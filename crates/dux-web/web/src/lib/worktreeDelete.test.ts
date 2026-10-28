import { describe, expect, it } from "vitest"

import { type Prose, proseText } from "@/lib/prose"
import { leftoverDeleteReport, worktreeDeleteReport } from "@/lib/worktreeDelete"

const text = (m: string | Prose) => (typeof m === "string" ? m : proseText(m))

describe("worktreeDeleteReport", () => {
  it("says the branch was kept when the server attempted nothing", () => {
    const r = worktreeDeleteReport("/wt/free", { branch: null })
    expect(r.tone).toBe("success")
    expect(text(r.message)).toContain("Its branch is still there")
    expect(r.sticky).toBe(false)
  })

  it("names the branch it deleted", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "deleted" },
    })
    expect(r.tone).toBe("success")
    expect(text(r.message)).toContain('deleted its branch "free"')
  })

  it("does not claim a deletion for a branch that was already gone", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "already_gone" },
    })
    expect(text(r.message)).toContain('"free" was already gone')
    expect(text(r.message)).not.toContain("deleted its branch")
  })

  // The verified lie: the toast reported the CHECKBOX. git refuses a branch
  // that is checked out elsewhere, and the branch survives, so the message has
  // to say the opposite of what it used to.
  it("reports a refusal honestly, with git's reason and a way out", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: {
        name: "free",
        outcome: "refused",
        reason: "error: cannot delete branch 'free' used by worktree at '/w'",
      },
    })
    expect(r.tone).toBe("warning")
    expect(text(r.message)).toContain(
      'git refused to delete its branch "free"',
    )
    expect(text(r.message)).toContain("used by worktree at '/w'.")
    expect(text(r.message)).toContain("git branch -D 'free'")
    expect(text(r.message)).not.toContain("and deleted its branch")
    // A leftover branch is recovered outside dux, so this one pins.
    expect(r.sticky).toBe(true)
  })

  it("still reads when git said nothing", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "refused", reason: "  " },
    })
    expect(text(r.message)).toContain("git gave no reason.")
  })

  // A future outcome word must not silently read as success.
  it("treats an unknown outcome as a refusal rather than a success", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "something-new" },
    })
    expect(r.tone).toBe("warning")
  })

  // A server that answers no body at all (an older build, a proxy that ate it)
  // must not make the client claim a branch deletion.
  it("claims nothing when there is no reply body", () => {
    const r = worktreeDeleteReport("/wt/free", null)
    expect(text(r.message)).toContain("Its branch is still there")
  })

  // A branch name can come from somebody else's pull request, and the
  // suggestion is a command the user is invited to paste: double quotes left
  // `$` and a closing quote live, so a crafted name ran its own command.
  it("single-quotes the branch in the suggested command and draws it as one chip", () => {
    const crafted = 'foo";touch${IFS}INJECTED;echo"'
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: crafted, outcome: "refused", reason: "nope" },
    })
    expect(Array.isArray(r.message)).toBe(true)
    const names = (r.message as Prose).flatMap((s) =>
      typeof s === "string" ? [] : [s.name],
    )
    expect(names).toContain(`git branch -D '${crafted}'`)
    expect(names).toContain(crafted)
  })

  it("closes and reopens the quotes around an apostrophe in the branch", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "it's", outcome: "refused", reason: "nope" },
    })
    expect(text(r.message)).toContain(`git branch -D 'it'\\''s'`)
  })
})

describe("leftoverDeleteReport", () => {
  // The user's decision: no toast from a left-over folder delete stays on
  // screen until closed, a warning included.
  it.each([true, false])("never pins the toast (warning: %s)", (warning) => {
    const r = leftoverDeleteReport({
      removed: !warning,
      warning,
      message: "Deleted.",
      segments: null,
    })
    expect(r.tone).toBe(warning ? "warning" : "success")
    expect(r).not.toHaveProperty("sticky")
  })
})
