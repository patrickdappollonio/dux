// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import { PrBanner } from "@/components/PrBanner"
import type { PrKeyView, PrView } from "@/lib/types"
import { sessionPrs } from "@/lib/pr"
import { stubMatchMedia } from "@/test/matchMedia"
import { type RawWorkspace, normalizeWorkspace } from "@/lib/workspaceApi"

// The real tooltip only mounts its popup into a portal on hover and needs a
// ResizeObserver, which jsdom lacks. Render its `content` inline instead so a
// test can assert what the banner's tooltip is wired to reveal, mirroring the
// pattern used in Sidebar.test.tsx.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: ReactNode
    content: ReactNode
  }) => (
    <>
      {children}
      <div data-testid="tooltip-content">{content}</div>
    </>
  ),
}))

function pr(overrides: Partial<PrView> = {}): PrView {
  return {
    number: 42,
    state: "open",
    title: "A very long pull request title that would truncate in the banner",
    url: "https://example.com/pr/42",
    overridden: false,
    host: "github.com",
    owner_repo: "acme/api",
    own_repo: true,
    ...overrides,
  }
}

function banner(
  prs: PrView[],
  cursor: PrKeyView | null = null,
  onCursor: (key: PrKeyView) => void = () => {},
  onManage: () => void = () => {},
) {
  return (
    <PrBanner prs={prs} cursor={cursor} onCursor={onCursor} onManage={onManage} />
  )
}

describe("PrBanner", () => {
  afterEach(() => {
    cleanup()
  })

  it("renders the visible #number once, not repeated in the tooltip", () => {
    render(banner([pr()]))

    // The number is visible in the banner itself.
    expect(screen.getByText("#42")).toBeTruthy()
    // The tooltip content carries only the title, not another "#42".
    const tooltip = screen.getByTestId("tooltip-content")
    expect(tooltip.textContent).not.toContain("#42")
  })

  it("carries the full PR title in the tooltip so a truncated title stays readable", () => {
    const longTitle =
      "A very long pull request title that would truncate in the banner"
    render(banner([pr({ title: longTitle })]))

    const tooltip = screen.getByTestId("tooltip-content")
    expect(tooltip.textContent).toBe(longTitle)
  })
})

describe("PrBanner with a title that arrived over the wire", () => {
  afterEach(() => {
    cleanup()
  })

  it("draws the title in its stored order, with no bidi control left in it", () => {
    const crafted = pr({ title: "Fix login \u202Eexe.txt\u202C" })
    const spine = normalizeWorkspace({
      projects: [],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          pr: crafted,
          prs: [crafted],
          prs_hidden: [pr({ number: 9, title: "Hidden \u2067one\u2069" })],
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as RawWorkspace)
    const session = spine.sessions[0]
    const { container } = render(banner(sessionPrs(session)))
    expect(container.textContent).toContain("Fix login exe.txt")
    expect(container.textContent).not.toMatch(/[\u202A-\u202E\u2066-\u2069\u200E\u200F\u061C]/)
    expect(session.prs_hidden?.map((p) => p.title)).toEqual(["Hidden one"])
  })
})

describe("PrBanner with several pull requests", () => {
  afterEach(() => {
    cleanup()
  })

  const a = pr({ number: 418, title: "Store every tab as a row", url: "https://g/418" })
  const b = pr({
    number: 77,
    title: "Allow the preview host",
    url: "https://g/77",
    owner_repo: "acme/infra",
    own_repo: false,
    attached: true,
    found: false,
  })
  const c = pr({ number: 415, state: "merged", title: "Promote the next tab", url: "https://g/415" })
  const keyOf = (p: PrView): PrKeyView => ({
    host: "github.com",
    owner_repo: p.owner_repo!,
    number: p.number,
  })

  it("has no counter or arrows with one pull request, and keeps the list button", () => {
    render(banner([a]))
    expect(screen.queryByText("1 of 1")).toBeNull()
    expect(screen.queryByLabelText("Previous pull request")).toBeNull()
    expect(screen.queryByLabelText("Next pull request")).toBeNull()
    expect(screen.getByLabelText("All pull requests")).toBeTruthy()
  })

  it("links the pull request at the cursor and counts its place", () => {
    render(banner([a, b, c], keyOf(b)))
    expect(screen.getByText("2 of 3")).toBeTruthy()
    expect(screen.getByRole("link").getAttribute("href")).toBe("https://g/77")
    expect(screen.getByText("acme/infra#77")).toBeTruthy()
  })

  it("wraps from the last to the first and from the first to the last", () => {
    const onCursor = vi.fn()
    render(banner([a, b, c], keyOf(c), onCursor))
    fireEvent.click(screen.getByLabelText("Next pull request"))
    expect(onCursor).toHaveBeenLastCalledWith(keyOf(a))
    cleanup()
    render(banner([a, b, c], null, onCursor))
    fireEvent.click(screen.getByLabelText("Previous pull request"))
    expect(onCursor).toHaveBeenLastCalledWith(keyOf(c))
  })

  it("keeps showing the stepped pull request when the list reorders, and falls back once it leaves", () => {
    const { rerender } = render(banner([a, b, c], keyOf(b)))
    rerender(banner([b, c, a], keyOf(b)))
    expect(screen.getByRole("link").getAttribute("href")).toBe("https://g/77")
    expect(screen.getByText("1 of 3")).toBeTruthy()
    rerender(banner([a, c], keyOf(b)))
    expect(screen.getByRole("link").getAttribute("href")).toBe("https://g/418")
  })

  it("colors a draft strip in the draft gray", () => {
    render(banner([pr({ state: "draft" })]))
    expect(screen.getByTestId("pr-banner").className).toContain("bg-dux-pr-draft-tint/10")
  })

  it("gives the counter and every control one height token, 40px on a coarse pointer", () => {
    render(banner([a, b, c]))
    const heights = [
      screen.getByText("1 of 3"),
      screen.getByLabelText("Previous pull request"),
      screen.getByLabelText("Next pull request"),
      screen.getByLabelText("All pull requests"),
    ].map((el) => el.className.split(/\s+/).filter((c) => /(^|:)h-/.test(c)).sort().join(" "))
    expect(new Set(heights).size).toBe(1)
    expect(heights[0]).toContain("pointer-coarse:h-10")
  })

  it("lists every pull request under the list button, and Pull requests… manages them", async () => {
    const media = stubMatchMedia()
    const onManage = vi.fn()
    try {
      render(banner([a, b, c], null, () => {}, onManage))
      await act(async () => {
        fireEvent.click(screen.getByLabelText("All pull requests"))
      })
      expect(screen.getByText("3 pull requests on this agent")).toBeTruthy()
      expect(screen.getByText("attached by you", { exact: false })).toBeTruthy()
      const manage = screen.getByText("Pull requests…")
      fireEvent.click(manage)
      expect(onManage).toHaveBeenCalled()
    } finally {
      media.restore()
    }
  })
})
