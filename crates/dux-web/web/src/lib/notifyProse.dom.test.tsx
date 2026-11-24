// @vitest-environment jsdom
import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeAll, describe, expect, it } from "vitest"

import { Toaster } from "@/components/ui/sonner"

import { chip, quotedChip } from "./prose"
import { notifyBusy, notifyError, notifyStatus, setStatusClearSeconds } from "./notify"

// A toast whose sentence names things draws each name through the shared inline
// code chip. Asserted against the real sonner, so what is checked is what a
// person sees.

beforeAll(() => {
  // jsdom ships no Pointer Events; sonner calls setPointerCapture on every
  // pointerdown. Same stub as the Toaster's own test.
  if (!Element.prototype.setPointerCapture) {
    Element.prototype.setPointerCapture = () => {}
    Element.prototype.releasePointerCapture = () => {}
    Element.prototype.hasPointerCapture = () => false
  }
})

afterEach(() => {
  setStatusClearSeconds(undefined)
  cleanup()
})

function toastEl(): HTMLElement {
  const el = document.querySelector("[data-sonner-toast]")
  if (!el) throw new Error("no toast rendered")
  return el as HTMLElement
}

function chips(el: HTMLElement): string[] {
  return [...el.querySelectorAll('code[data-slot="inline-code"]')].map(
    (c) => c.textContent ?? "",
  )
}

describe("names in a toast", () => {
  it("draws every name of a structured engine status as a chip, without its quotes", async () => {
    render(<Toaster />)
    act(() => {
      notifyStatus(
        "info",
        ["Checked out ", quotedChip("main"), " for project ", quotedChip("app"), "."],
        { id: "checkout" },
      )
    })
    await waitFor(() => toastEl())
    expect(chips(toastEl())).toEqual(["main", "app"])
    expect(toastEl().textContent).toContain("Checked out main for project app.")
    expect(toastEl().textContent).not.toContain('"main"')
  })

  it("draws a plain sentence as the text it always was", async () => {
    render(<Toaster />)
    act(() => {
      notifyStatus("warning", 'Plain words about "main".', { id: "plain" })
    })
    await waitFor(() => toastEl())
    expect(chips(toastEl())).toEqual([])
    expect(toastEl().textContent).toContain('Plain words about "main".')
  })

  it("shows a file name whose backticks pair up exactly as typed", async () => {
    render(<Toaster />)
    const text = "refusing to rename, destination already exists: /w/a`b`c.txt"
    act(() => {
      notifyError(text, { id: "rename" })
    })
    await waitFor(() => toastEl())
    expect(chips(toastEl())).toEqual([])
    expect(toastEl().textContent).toContain(text)
  })

  it("shows a parser's error text exactly as typed, backticks and all", async () => {
    render(<Toaster />)
    const text = "Config reload failed: TOML parse error at line 3, key = `x`, expected `\"`, `'`"
    act(() => {
      notifyError(text, { id: "reload" })
    })
    await waitFor(() => toastEl())
    expect(chips(toastEl())).toEqual([])
    expect(toastEl().textContent).toContain(text)
  })

  it("chips the names of a spinner too", async () => {
    render(<Toaster />)
    act(() => {
      notifyBusy(["Pulling ", quotedChip("main"), "…"], { id: "pull" })
    })
    await waitFor(() => toastEl())
    expect(chips(toastEl())).toEqual(["main"])
  })

  it("lets a toast the browser raises itself name things as chips", async () => {
    render(<Toaster />)
    act(() => {
      notifyError(["Could not save ", chip("src/a b.ts"), "."])
    })
    await waitFor(() => toastEl())
    expect(chips(toastEl())).toEqual(["src/a b.ts"])
  })

  it("raises nothing for a sentence with no words", async () => {
    render(<Toaster />)
    act(() => {
      notifyError([])
    })
    expect(document.querySelector("[data-sonner-toast]")).toBeNull()
  })
})
