// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { Lock } from "lucide-react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { Notice } from "@/components/Notice"
import { StaleSaveNotice } from "@/components/StaleSaveNotice"

afterEach(cleanup)

describe("Notice", () => {
  it("tells a screen reader by `announce`, and by tone when it is left unset", () => {
    const { rerender } = render(
      <Notice shape="box" tone="danger">
        words
      </Notice>,
    )
    expect(screen.getByRole("alert").textContent).toBe("words")
    for (const tone of ["warning", "info", "neutral"] as const) {
      rerender(
        <Notice shape="box" tone={tone}>
          words
        </Notice>,
      )
      expect(screen.queryByRole("alert")).toBeNull()
      expect(screen.getByRole("status").textContent).toBe("words")
    }
    // A warning that is a question to answer interrupts; a danger notice that is
    // there when the page renders is a note, read by nobody until visited.
    rerender(
      <Notice shape="box" tone="warning" announce="alert">
        words
      </Notice>,
    )
    expect(screen.getByRole("alert")).toBeTruthy()
    rerender(
      <Notice shape="box" tone="danger" announce="static">
        words
      </Notice>,
    )
    expect(screen.queryByRole("alert")).toBeNull()
    expect(screen.queryByRole("status")).toBeNull()
    expect(screen.getByRole("note").textContent).toBe("words")
  })

  it("marks a strip, and only a strip, for the phone's toasts to sit below", () => {
    const { rerender } = render(
      <Notice shape="strip" tone="warning" icon={Lock}>
        words
      </Notice>,
    )
    expect(screen.getByRole("status").hasAttribute("data-toast-clear")).toBe(true)
    rerender(
      <Notice shape="box" tone="warning" icon={Lock}>
        words
      </Notice>,
    )
    expect(screen.getByRole("status").hasAttribute("data-toast-clear")).toBe(false)
  })

  it("runs a lead sentence into its words and holds a row under them", () => {
    render(
      <Notice shape="banner" tone="danger" lead="Heads up." below={<button>Extra</button>}>
        the rest
      </Notice>,
    )
    expect(screen.getByRole("alert").textContent).toContain("Heads up. the rest")
    expect(screen.getByRole("alert").querySelector("strong")?.textContent).toBe("Heads up.")
  })

  it("names itself by its title and runs its buttons", () => {
    const onSelect = vi.fn()
    const onDismiss = vi.fn()
    render(
      <Notice
        shape="strip"
        tone="warning"
        title="Heading"
        actions={[{ label: "Do it", onSelect }]}
        onDismiss={onDismiss}
      >
        words
      </Notice>,
    )
    expect(screen.getByRole("status", { name: "Heading" })).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Do it" }))
    fireEvent.click(screen.getByRole("button", { name: "Dismiss this notice" }))
    expect(onSelect).toHaveBeenCalledTimes(1)
    expect(onDismiss).toHaveBeenCalledTimes(1)
  })
})

describe("StaleSaveNotice", () => {
  it("is an alert whose Keep editing has focus and whose reload is the other button", () => {
    const keep = vi.fn()
    const reload = vi.fn()
    render(
      <StaleSaveNotice
        message="It changed."
        reloadLabel="Reload the file"
        onKeepEditing={keep}
        onReload={reload}
      />,
    )
    expect(screen.getByRole("alert").textContent).toContain("It changed.")
    const keepButton = screen.getByRole("button", { name: "Keep editing" })
    expect(document.activeElement).toBe(keepButton)
    fireEvent.click(keepButton)
    fireEvent.click(screen.getByRole("button", { name: "Reload the file" }))
    expect(keep).toHaveBeenCalledTimes(1)
    expect(reload).toHaveBeenCalledTimes(1)
  })
})
