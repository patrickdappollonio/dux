import { describe, expect, it } from "vitest"

import { projectBranchDisplay } from "./projectBranch"

describe("projectBranchDisplay", () => {
  it("renders a leading branch muted with no tooltip", () => {
    const d = projectBranchDisplay({
      current_branch: "main",
      branch_status: "leading",
    })
    expect(d).toEqual({ branch: "main", warn: false, tooltip: null })
  })

  it("renders an unknown-status branch muted with no tooltip", () => {
    const d = projectBranchDisplay({
      current_branch: "main",
      branch_status: "unknown",
    })
    expect(d).toEqual({ branch: "main", warn: false, tooltip: null })
  })

  it("warns and explains a non-leading branch", () => {
    const d = projectBranchDisplay({
      current_branch: "feature/x",
      branch_status: "not_leading",
    })
    expect(d?.branch).toBe("feature/x")
    expect(d?.warn).toBe(true)
    expect(d?.tooltip).toBe(
      "On feature/x — this doesn't appear to be the project's leading branch.",
    )
  })

  it("renders nothing for an empty branch", () => {
    expect(
      projectBranchDisplay({ current_branch: "", branch_status: "unknown" }),
    ).toBeNull()
  })

  it("renders nothing for a whitespace-only branch", () => {
    expect(
      projectBranchDisplay({ current_branch: "   ", branch_status: "leading" }),
    ).toBeNull()
  })
})
