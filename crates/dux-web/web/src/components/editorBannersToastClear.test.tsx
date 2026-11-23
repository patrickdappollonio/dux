// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { EncodingNotice } from "@/components/EncodingNotice"
import { FileDiskBanner } from "@/components/FileDiskBanner"
import { prose } from "@/lib/prose"

// Both sit flush under the editor's tab strip with buttons on them, so a
// phone's toasts start below them as they do below the strip.
describe("the editor's in-pane notices", () => {
  afterEach(cleanup)

  it("mark the disk banner as chrome a phone toast clears", () => {
    render(
      <FileDiskBanner
        state="changed"
        path="a.ts"
        dirty={false}
        onReload={() => {}}
        onDismiss={() => {}}
        onCloseTab={() => {}}
      />,
    )
    expect(screen.getByRole("status").hasAttribute("data-toast-clear")).toBe(true)
  })

  it("mark the encoding notice as chrome a phone toast clears", () => {
    render(
      <EncodingNotice
        notice={{ kind: "mixed_eol", message: prose`Mixed line endings.`, dismissable: true }}
        onDismiss={() => {}}
      />,
    )
    expect(screen.getByRole("status").hasAttribute("data-toast-clear")).toBe(true)
  })
})
