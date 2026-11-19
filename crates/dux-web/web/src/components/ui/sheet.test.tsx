// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

// A sheet closes on Back through the store, so the real store is loaded over a
// history that has an entry for it.
installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { Sheet, SheetContent, SheetTitle } from "./sheet"

afterEach(cleanup)

describe("SheetContent close button touch size", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("extends the corner close button's tap area at phone width and on a coarse pointer without growing or moving its visible box", () => {
    render(
      <Sheet open>
        <SheetContent side="bottom">
          <SheetTitle>Title</SheetTitle>
        </SheetContent>
      </Sheet>,
    )
    const classes = Array.from(
      document.querySelector('[data-slot="sheet-close"]')?.classList ?? [],
    )
    expect(
      classes,
      "the 28px close button needs an invisible margin at phone width and under a coarse pointer, and none otherwise",
    ).toEqual(
      expect.arrayContaining([
        "after:hidden",
        "max-md:after:block",
        "pointer-coarse:after:block",
        "after:-inset-1.5",
      ]),
    )
    expect(
      classes.filter((c) => /size-1[01]/.test(c)),
      "growing the box moves the X glyph with it, so the box stays 28px",
    ).toEqual([])
    expect(classes, "the dialog's corner X sits 8px from the corner, and so does the sheet's").toEqual(
      expect.arrayContaining(["top-2", "right-2", "size-7"]),
    )
  })
})
