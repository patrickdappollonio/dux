// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import type { DuxState } from "@/lib/store"

// Override `useDux` so the dialog reads our seeded bootstrap, and replace the
// store actions the dialog dispatches with spies so we can assert the exact
// body it posts. The rest of the real store exports stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    setInstanceIdentity: vi.fn(),
    closeCustomizeWebapp: vi.fn(),
    setChangesPaneVisibility: vi.fn(),
  }
})

// The real tooltip only mounts its popup on hover and needs a ResizeObserver that
// jsdom lacks; render its trigger children directly so the swatch buttons exist.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({ children }: { children: ReactNode }) => <>{children}</>,
}))

// The real store boots on import (localStorage + bootstrap fetch). jsdom doesn't
// provide those as bare globals, so stub them before the component loads.
function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()
const { CustomizeWebappDialog } = await import("./CustomizeWebappDialog")
const store = await import("@/lib/store")
const setInstanceIdentity = vi.mocked(store.setInstanceIdentity)
const closeCustomizeWebapp = vi.mocked(store.closeCustomizeWebapp)
const setChangesPaneVisibility = vi.mocked(store.setChangesPaneVisibility)

function seed(bootstrap: {
  title?: string
  favicon?: string
  show_changes_pane?: boolean
}) {
  mockState = {
    customizeWebappOpen: true,
    changesPaneOverride: null,
    bootstrap,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  setInstanceIdentity.mockClear()
  closeCustomizeWebapp.mockClear()
  setChangesPaneVisibility.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("CustomizeWebappDialog", () => {
  it("posts the edited title and the picked tint colour on Save", () => {
    seed({ title: "old instance", favicon: "" })
    render(<CustomizeWebappDialog />)

    const input = screen.getByPlaceholderText("dux") as HTMLInputElement
    fireEvent.change(input, { target: { value: "prod dux" } })
    fireEvent.click(screen.getByRole("button", { name: "Blue" }))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(setInstanceIdentity).toHaveBeenCalledTimes(1)
    expect(setInstanceIdentity).toHaveBeenCalledWith({
      title: "prod dux",
      favicon: "blue",
    })
    expect(closeCustomizeWebapp).toHaveBeenCalled()
  })

  it("posts empty strings (reset-to-default) on Reset to default", () => {
    seed({ title: "prod dux", favicon: "amber" })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }))

    expect(setInstanceIdentity).toHaveBeenCalledTimes(1)
    expect(setInstanceIdentity).toHaveBeenCalledWith({ title: "", favicon: "" })
  })

  it("selects the empty favicon when the Original swatch is chosen", () => {
    seed({ title: "old instance", favicon: "amber" })
    render(<CustomizeWebappDialog />)

    // Move off the seeded colour, then back to Original, and confirm Save carries
    // the empty favicon (the bundled full-colour duck).
    fireEvent.click(screen.getByRole("button", { name: "Blue" }))
    fireEvent.click(screen.getByRole("button", { name: "Original" }))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(setInstanceIdentity).toHaveBeenCalledWith({
      title: "old instance",
      favicon: "",
    })
  })

  it("saves on Enter in the title input without a full-page reload", () => {
    seed({ title: "old instance", favicon: "" })
    render(<CustomizeWebappDialog />)

    const input = screen.getByPlaceholderText("dux") as HTMLInputElement
    fireEvent.change(input, { target: { value: "renamed" } })
    // Enter is intercepted (preventDefault) so it saves instead of submitting a
    // form / triggering navigation.
    const event = fireEvent.keyDown(input, { key: "Enter" })
    // fireEvent returns false when a handler called preventDefault.
    expect(event).toBe(false)

    expect(setInstanceIdentity).toHaveBeenCalledTimes(1)
    expect(setInstanceIdentity).toHaveBeenCalledWith({
      title: "renamed",
      favicon: "",
    })
  })

  it("renders the Changes pane checkbox checked when show_changes_pane is true", () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    render(<CustomizeWebappDialog />)

    const checkbox = screen.getByRole("checkbox")
    expect(checkbox.getAttribute("aria-checked")).toBe("true")
  })

  it("renders the Changes pane checkbox unchecked when show_changes_pane is false", () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: false })
    render(<CustomizeWebappDialog />)

    const checkbox = screen.getByRole("checkbox")
    expect(checkbox.getAttribute("aria-checked")).toBe("false")
  })

  it("unchecking the Changes pane checkbox and saving persists it, alongside the identity", () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("checkbox"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(setChangesPaneVisibility).toHaveBeenCalledTimes(1)
    expect(setChangesPaneVisibility).toHaveBeenCalledWith(false)
    expect(setInstanceIdentity).toHaveBeenCalledTimes(1)
  })

  it("does not persist the Changes pane preference when Save is clicked without touching it", () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(setChangesPaneVisibility).not.toHaveBeenCalled()
  })

  it("resets the Changes pane to visible on Reset to default when it was hidden", () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: false })
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }))

    expect(setChangesPaneVisibility).toHaveBeenCalledTimes(1)
    expect(setChangesPaneVisibility).toHaveBeenCalledWith(true)
  })
})
