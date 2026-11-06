import { describe, expect, it } from "vitest"

import {
  prActivity,
  prAriaLabel,
  prBadgeTooltip,
  prBannerClass,
  prCursorIndex,
  prName,
  prOrigin,
  prRowAction,
  renameWarningPr,
  stepPrIndex,
  prIconClass,
  prIconHoverClass,
  prStateLabel,
} from "@/lib/pr"
import type { PrView } from "@/lib/types"

function view(overrides: Partial<PrView> = {}): PrView {
  return {
    number: 418,
    state: "open",
    title: "Store every tab as a row with a stable id",
    url: "https://github.com/acme/api/pull/418",
    overridden: false,
    host: "github.com",
    owner_repo: "acme/api",
    own_repo: true,
    found: true,
    attached: false,
    ...overrides,
  }
}

describe("prStateLabel", () => {
  it("maps known states to their TUI lowercase words", () => {
    expect(prStateLabel("open")).toBe("open")
    expect(prStateLabel("draft")).toBe("draft")
    expect(prStateLabel("merged")).toBe("merged")
    expect(prStateLabel("closed")).toBe("closed")
  })

  it("falls back to 'unknown' for any unrecognized state", () => {
    expect(prStateLabel("queued")).toBe("unknown")
    expect(prStateLabel("")).toBe("unknown")
  })
})

describe("prIconClass", () => {
  it("tints the icon green/gray/purple/red per state", () => {
    expect(prIconClass("open")).toContain("text-dux-pr-open")
    expect(prIconClass("draft")).toContain("text-dux-pr-draft")
    expect(prIconClass("merged")).toContain("text-dux-pr-merged")
    expect(prIconClass("closed")).toContain("text-dux-pr-closed")
  })

  it("uses a neutral tint for unknown states", () => {
    expect(prIconClass("queued")).toContain("text-muted-foreground")
  })
})

describe("prIconHoverClass", () => {
  it("provides an explicit same-hue hover for every known state", () => {
    expect(prIconHoverClass("open")).toContain("hover:text-dux-pr-open-hover")
    expect(prIconHoverClass("open")).toContain("hover:bg-dux-pr-open-tint/15")
    expect(prIconHoverClass("merged")).toContain("hover:text-dux-pr-merged-hover")
    expect(prIconHoverClass("closed")).toContain("hover:text-dux-pr-closed-hover")
    expect(prIconHoverClass("draft")).toContain("hover:text-dux-pr-draft-hover")
    expect(prIconHoverClass("draft")).toContain("hover:bg-dux-pr-draft-tint/15")
  })

  it("never emits a near-white hover surface that would wash the icon out", () => {
    for (const state of ["open", "merged", "closed", "draft"]) {
      const cls = prIconHoverClass(state)
      expect(cls).not.toContain("bg-white")
      expect(cls).not.toContain("bg-background")
    }
  })
})

describe("prBannerClass", () => {
  it("produces a soft state-colored strip per state", () => {
    expect(prBannerClass("open")).toContain("bg-dux-pr-open-tint/10")
    expect(prBannerClass("open")).toContain("border-dux-pr-open-tint/30")
    expect(prBannerClass("merged")).toContain("bg-dux-pr-merged-tint/10")
    expect(prBannerClass("closed")).toContain("bg-dux-pr-closed-tint/10")
    expect(prBannerClass("draft")).toContain("bg-dux-pr-draft-tint/10")
    expect(prBannerClass("draft")).toContain("text-dux-pr-draft")
  })

  it("falls back to a neutral strip for unknown states", () => {
    expect(prBannerClass("queued")).toContain("bg-muted")
  })
})

describe("prAriaLabel", () => {
  it("spells the words out and speaks the state as a clause", () => {
    expect(prAriaLabel(123, "open")).toBe("Pull request #123, open")
    expect(prAriaLabel(7, "merged")).toBe("Pull request #7, merged")
    expect(prAriaLabel(7, "closed")).toBe("Pull request #7, closed")
    expect(prAriaLabel(7, "draft")).toBe("Pull request #7, draft")
  })

  it("says nothing about a state it does not know", () => {
    // "Pull request #7, unknown" would be worse than silence: it announces a
    // word the listener has to decide is meaningless.
    expect(prAriaLabel(7, "queued")).toBe("Pull request #7")
  })
})

describe("prName", () => {
  it("names one in the agent's own repository by number alone", () => {
    expect(prName(view())).toBe("#418")
  })

  it("names one from another repository with its repository", () => {
    expect(prName(view({ number: 77, owner_repo: "acme/infra", own_repo: false }))).toBe(
      "acme/infra#77",
    )
  })
})

describe("prCursorIndex", () => {
  const a = view({ number: 418 })
  const b = view({ number: 77, owner_repo: "acme/infra", own_repo: false })
  const c = view({ number: 415, state: "merged" })

  it("follows the pull request the cursor names when the list reorders", () => {
    const key = { host: "github.com", owner_repo: "acme/infra", number: 77 }
    expect(prCursorIndex([a, b, c], key)).toBe(1)
    expect(prCursorIndex([b, c, a], key)).toBe(0)
  })

  it("falls back to the first when the named one has left the list", () => {
    const key = { host: "github.com", owner_repo: "acme/api", number: 402 }
    expect(prCursorIndex([a, b, c], key)).toBe(0)
    expect(prCursorIndex([a, b, c], null)).toBe(0)
  })

  it("tells two repositories' pull requests of one number apart", () => {
    const other = view({ number: 418, owner_repo: "acme/infra", own_repo: false })
    const key = { host: "github.com", owner_repo: "acme/infra", number: 418 }
    expect(prCursorIndex([a, other], key)).toBe(1)
  })
})

describe("stepPrIndex", () => {
  it("steps forward and back, wrapping at both ends", () => {
    expect(stepPrIndex(3, 0, 1)).toBe(1)
    expect(stepPrIndex(3, 2, 1)).toBe(0)
    expect(stepPrIndex(3, 0, -1)).toBe(2)
  })
})

describe("prRowAction", () => {
  it("hides a found pull request and detaches an attached one", () => {
    expect(prRowAction(view({ found: true, attached: false }))).toBe("hide")
    expect(prRowAction(view({ found: false, attached: true }))).toBe("detach")
  })

  it("detaches one that is both found and attached", () => {
    expect(prRowAction(view({ found: true, attached: true }))).toBe("detach")
  })
})

describe("prOrigin", () => {
  it("names the branch a found pull request is on", () => {
    expect(prOrigin(view({ head_branch: "store-tab-rows" }))).toEqual({
      kind: "branch",
      name: "store-tab-rows",
    })
  })

  it("says the user attached one dux did not find", () => {
    expect(
      prOrigin(view({ found: false, attached: true, head_branch: "main" })),
    ).toEqual({ kind: "attached" })
  })
})

describe("prActivity", () => {
  const now = Date.parse("2026-11-06T12:00:00Z")

  it("says when an open one was last updated", () => {
    expect(prActivity(view({ updated_at: "2026-11-06T10:00:00Z" }), now)).toBe(
      "updated 2 hours ago",
    )
  })

  it("says when a merged one merged and a closed one closed", () => {
    expect(
      prActivity(
        view({
          state: "merged",
          updated_at: "2026-11-06T11:00:00Z",
          merged_at: "2026-11-03T12:00:00Z",
        }),
        now,
      ),
    ).toBe("merged 3 days ago")
    expect(
      prActivity(view({ state: "closed", closed_at: "2026-11-05T12:00:00Z" }), now),
    ).toBe("closed yesterday")
  })

  it("says nothing when GitHub has not given a time", () => {
    expect(prActivity(view(), now)).toBeNull()
  })
})

describe("prBadgeTooltip", () => {
  it("names the shown pull request and counts the rest", () => {
    expect(prBadgeTooltip([view(), view({ number: 77 }), view({ number: 415 })])).toBe(
      "#418 open · Store every tab as a row with a stable id. 2 more pull requests; the banner steps through them.",
    )
  })

  it("says one more in the singular", () => {
    expect(prBadgeTooltip([view({ state: "draft" }), view({ number: 77 })])).toBe(
      "#418 draft · Store every tab as a row with a stable id. 1 more pull request; the banner steps through them.",
    )
  })
})

describe("renameWarningPr", () => {
  const key = (number: number) => ({ host: "github.com", owner_repo: "acme/api", number })

  it("names an open or draft pull request on the branch the branch read puts on it", () => {
    const draft = view({ number: 9, state: "draft", head_branch: "feat/old" })
    expect(
      renameWarningPr({ visible: [view({ state: "merged" }), draft], hidden: [], branch: "feat/new", onBranch: [key(9)] }),
    ).toBe(9)
  })

  it("names none when the only open one is on another branch", () => {
    const elsewhere = view({ number: 5, head_branch: "other" })
    expect(renameWarningPr({ visible: [elsewhere], hidden: [], branch: "feat", onBranch: [] })).toBeNull()
    expect(renameWarningPr({ visible: [elsewhere], hidden: [], branch: "feat", onBranch: null })).toBeNull()
  })

  it("matches the branch's own name before the branch read has answered", () => {
    const hidden = view({ number: 6, head_branch: "feat" })
    expect(renameWarningPr({ visible: [], hidden: [hidden], branch: "feat", onBranch: null })).toBe(6)
  })

  it("falls back to the shown open one while no pull request names its branch", () => {
    const shown = view({ number: 7, head_branch: undefined })
    expect(renameWarningPr({ visible: [shown], hidden: [], branch: "feat", onBranch: [] })).toBe(7)
  })
})
