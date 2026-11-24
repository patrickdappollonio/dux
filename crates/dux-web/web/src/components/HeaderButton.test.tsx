// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react"
import { Plus } from "lucide-react"
import { afterEach, describe, expect, it } from "vitest"

import { EllipsisMenuTrigger } from "@/components/EllipsisMenuTrigger"
import { FoldedAway, HeaderButton } from "@/components/HeaderButton"
import { HEADER_BOX } from "@/components/ellipsisPlacements"
import { DropdownMenu } from "@/components/ui/dropdown-menu"

afterEach(cleanup)

const classes = (node: HTMLElement) => node.className.split(/\s+/)

describe("HeaderButton", () => {
  it("is one outline button at one height, on a computer, a phone and under a finger", () => {
    render(
      <>
        <HeaderButton icon={<Plus />} label="Labelled" />
        <HeaderButton icon={<Plus />} label="Word" words="computer" />
        <HeaderButton icon={<Plus />} label="Narrow" words="wide" />
      </>,
    )
    for (const name of ["Labelled", "Word", "Narrow"]) {
      const button = screen.getByText(name).closest("button")!
      expect(classes(button), name).toEqual(
        expect.arrayContaining(["border-border", "h-8", "max-md:h-10", "pointer-coarse:h-10"]),
      )
      // The icon is the primitive's 16px, with its 6px gap to the word.
      expect(classes(button), name).toContain("gap-1.5")
    }
  })

  it("is the ⋯'s own box when it has no word, so the two sit as one cluster", () => {
    render(
      <DropdownMenu>
        <HeaderButton icon={<Plus />} aria-label="Icon only" />
        <EllipsisMenuTrigger placement="header" aria-label="More" />
      </DropdownMenu>,
    )
    const box = (label: string) =>
      classes(screen.getByLabelText(label)).filter((c) => /size-|max-md:size|coarse:size/.test(c))
    expect(box("Icon only")).toEqual(box("More"))
    expect(box("More")).toContain("size-8")
  })

  // Where a word gives way to its icon the button is the `⋯`'s box, so a row of
  // header buttons is even. The sizes are read from `HEADER_BOX`, so the box
  // moving moves what this expects: each of its sizes, at the width where the
  // word is gone, must be a width of the same number on the button.
  it.each([
    ["computer", "max-md"],
    ["wide", "max-xl"],
  ] as const)("has the ⋯'s box where the %s word is gone", (words, gone) => {
    render(<HeaderButton icon={<Plus />} label="Word" words={words} />)
    const button = classes(screen.getByRole("button", { name: "Word" }))
    const sizes = HEADER_BOX.split(/\s+/).flatMap((token) => {
      const m = token.match(/^(?:([\w-]+):)?size-(\d+)$/)
      return m ? [{ when: m[1], n: m[2] }] : []
    })
    // The plain size is the box where the word is gone only when that is above
    // a phone, where the phone's own size takes over; a coarse pointer's is
    // added only where the word is gone above a phone as well.
    const expected = sizes.flatMap(({ when, n }) => {
      if (when === undefined) return gone === "max-xl" ? [`${gone}:w-${n}`] : []
      if (when === "max-md") return [`max-md:w-${n}`]
      return gone === "max-xl" ? [`${when}:${gone}:w-${n}`] : []
    })
    expect(expected.length).toBeGreaterThan(0)
    for (const token of expected) expect(button, token).toContain(token)
    // No padding is left to push the icon off the middle of the box.
    expect(button).toContain(`${gone}:px-0`)
  })

  it("is named by its word, or by its aria-label when it shows none", () => {
    render(
      <>
        <HeaderButton icon={<Plus />} label="Word" words="computer" />
        <HeaderButton icon={<Plus />} aria-label="Named only" />
      </>,
    )
    expect(screen.getByRole("button", { name: "Word" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Named only" })).toBeTruthy()
  })

  it("keeps its word for a screen reader where it hides it", () => {
    render(<HeaderButton icon={<Plus />} label="Editor" words="computer" />)
    const word = screen.getByText("Editor")
    expect(word.className).toContain("max-md:sr-only")
    expect(screen.getByRole("button", { name: "Editor" })).toBeTruthy()
  })

  it("is out of sight and reach while folded away, and adds no box otherwise", () => {
    render(
      <>
        <FoldedAway folded>
          <HeaderButton icon={<Plus />} label="Folded" />
        </FoldedAway>
        <FoldedAway folded={false}>
          <HeaderButton icon={<Plus />} label="Shown" />
        </FoldedAway>
      </>,
    )
    expect(screen.getByText("Folded").closest(".invisible")).toBeTruthy()
    expect(screen.getByText("Shown").closest(".invisible")).toBeNull()
    expect(screen.getByText("Shown").closest("button")!.parentElement!.className).toBe("contents")
  })

  it("wears the filled variant only when it is the header's primary action", () => {
    render(
      <>
        <HeaderButton icon={<Plus />} label="Save" primary />
        <HeaderButton icon={<Plus />} label="Other" />
      </>,
    )
    expect(classes(screen.getByText("Save").closest("button")!)).toContain("bg-primary")
    expect(classes(screen.getByText("Other").closest("button")!)).not.toContain("bg-primary")
  })
})
