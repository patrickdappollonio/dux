// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { FileDiskBanner } from "./FileDiskBanner"
import { SaveConflictDialog } from "./SaveConflictDialog"
import { reopenedNotice } from "./useEditorFileReads"
import { proseText } from "@/lib/prose"

if (!Element.prototype.getAnimations) {
  Element.prototype.getAnimations = () => []
}

describe("a file with an operation out", () => {
  afterEach(cleanup)

  /// A delayed save answer and a reload confirmed meanwhile are two
  /// operations on one file; the banner's Reload waits for the save.
  it("offers no reload from the disk banner", () => {
    render(
      <FileDiskBanner
        state="changed"
        path="a.txt"
        dirty
        busy
        onReload={() => {}}
        onDismiss={() => {}}
        onCloseTab={() => {}}
      />,
    )
    expect(
      (screen.getByRole("button", { name: /reload from disk/i }) as HTMLButtonElement).disabled,
    ).toBe(true)
  })

  it("offers neither overwrite nor reload from the conflict dialog", () => {
    render(
      <SaveConflictDialog
        target={{
          tabId: "t",
          path: "a.txt",
          body: "x",
          encoding: { encoding: "UTF-8", bom: false },
          deleted: false,
        }}
        busy
        onOverwrite={() => {}}
        onReload={() => {}}
        onClose={() => {}}
      />,
    )
    expect(
      (screen.getByRole("button", { name: /^overwrite$/i }) as HTMLButtonElement).disabled,
    ).toBe(true)
    expect(
      (screen.getByRole("button", { name: /reload from disk/i }) as HTMLButtonElement).disabled,
    ).toBe(true)
  })
})

describe("what a reopen says it did", () => {
  const file = { path: "a", binary: false, content: "", encoding: "Shift_JIS" }

  it("says a file read-only for where it lives stays read-only", () => {
    const said = proseText(reopenedNotice(".git/config", "Shift_JIS", { ...file, path_read_only: true, read_only: true }))
    expect(said).toContain("stays read-only")
    expect(said).not.toContain("A save now writes")
  })

  it("says a file its encoding cannot write back is read-only", () => {
    const said = proseText(reopenedNotice("a", "Shift_JIS", { ...file, read_only: true, unwritable: "round_trip" }))
    expect(said).toContain("read-only")
    expect(said).not.toContain("A save now writes")
  })

  it("says a save now writes in it, only when one does", () => {
    expect(proseText(reopenedNotice("a", "Shift_JIS", file))).toContain("A save now writes")
  })
})
