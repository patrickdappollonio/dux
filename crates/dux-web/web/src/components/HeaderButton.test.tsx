// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react"
import { Plus } from "lucide-react"
import { afterEach, describe, expect, it } from "vitest"

import { EllipsisMenuTrigger } from "@/components/EllipsisMenuTrigger"
import { HeaderButton } from "@/components/HeaderButton"
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

  it("keeps its word for a screen reader where it hides it", () => {
    render(<HeaderButton icon={<Plus />} label="Editor" words="computer" />)
    const word = screen.getByText("Editor")
    expect(word.className).toContain("max-md:sr-only")
    expect(screen.getByRole("button", { name: "Editor" })).toBeTruthy()
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
