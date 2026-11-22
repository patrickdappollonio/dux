import { describe, expect, it } from "vitest"

import { createAgentFormView } from "./createAgentDialogView"

// A held Create says why on hover, and a lookup in flight holds it without a
// reason because the button shows a spinner instead.
describe("why Create is held", () => {
  it.each([
    ["new", "bad..name", "", false, false, "That name is not one git accepts for a branch."],
    ["fork", "", "", false, false, "Type a name for the fork first."],
    ["pr", "", "", false, false, "Enter the pull request first."],
    ["new", "", "", false, true, "Tick Create a review copy to go ahead."],
    ["pr", "", "12", true, false, null],
    ["new", "", "", false, false, null],
  ] as const)("%s with name %j and pull request %j", (kind, draft, pr, resolving, review, why) => {
    const form = createAgentFormView(kind, draft, pr, resolving, review)
    expect(form.submitRefusal).toBe(why)
    expect(form.submitDisabled).toBe(why !== null || resolving)
  })
})
