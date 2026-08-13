import { describe, expect, it } from "vitest"

import { worktreeDeleteReport } from "@/lib/worktreeDelete"

describe("worktreeDeleteReport", () => {
  it("says the branch was kept when the server attempted nothing", () => {
    const r = worktreeDeleteReport("/wt/free", { branch: null })
    expect(r.tone).toBe("success")
    expect(r.message).toContain("Its branch is still there")
    expect(r.sticky).toBe(false)
  })

  it("names the branch it deleted", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "deleted" },
    })
    expect(r.tone).toBe("success")
    expect(r.message).toContain('deleted its branch "free"')
  })

  it("does not claim a deletion for a branch that was already gone", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "already_gone" },
    })
    expect(r.message).toContain('"free" was already gone')
    expect(r.message).not.toContain("deleted its branch")
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
    expect(r.message).toContain("git refused to delete its branch \"free\"")
    expect(r.message).toContain("used by worktree at '/w'.")
    expect(r.message).toContain('git branch -D "free"')
    expect(r.message).not.toContain("and deleted its branch")
    // A leftover branch is recovered outside dux, so this one pins.
    expect(r.sticky).toBe(true)
  })

  it("still reads when git said nothing", () => {
    const r = worktreeDeleteReport("/wt/free", {
      branch: { name: "free", outcome: "refused", reason: "  " },
    })
    expect(r.message).toContain("git gave no reason.")
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
    expect(r.message).toContain("Its branch is still there")
  })
})
