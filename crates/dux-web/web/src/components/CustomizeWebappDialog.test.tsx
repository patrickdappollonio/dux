// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
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
  // The two persist spies resolve true (success) by default so save()/reset()
  // reach their close; individual tests override with false to pin the
  // stays-open-on-failure behavior.
  setInstanceIdentity.mockClear().mockResolvedValue(true)
  closeCustomizeWebapp.mockClear()
  setChangesPaneVisibility.mockClear().mockResolvedValue(true)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("CustomizeWebappDialog", () => {
  it("posts the edited title and the picked tint colour on Save", async () => {
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
    // The close now waits for the persist promises to settle.
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
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

  it("stays open when a persist fails so the user can retry", async () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    setChangesPaneVisibility.mockResolvedValue(false)
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("checkbox"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    // Both writes fired, but the failed one keeps the dialog open. The Save
    // button re-enabling proves save()'s promise chain fully settled (its
    // `finally` ran), so the no-close assertion is not a premature negative.
    await waitFor(() => expect(setChangesPaneVisibility).toHaveBeenCalled())
    await waitFor(() =>
      expect(
        (screen.getByRole("button", { name: "Save" }) as HTMLButtonElement)
          .disabled,
      ).toBe(false),
    )
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()
  })

  it("an untouched checkbox tracks a concurrent client's toggle and never writes it back", async () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    const { rerender } = render(<CustomizeWebappDialog />)
    expect(screen.getByRole("checkbox").getAttribute("aria-checked")).toBe(
      "true",
    )

    // Another connected client hides the pane while this dialog is open: the
    // refetched bootstrap flows into the store, and the untouched checkbox
    // must follow it instead of freezing at its open-time value.
    seed({ title: "old instance", favicon: "", show_changes_pane: false })
    rerender(<CustomizeWebappDialog />)
    expect(screen.getByRole("checkbox").getAttribute("aria-checked")).toBe(
      "false",
    )

    // Saving without touching the checkbox must not write the pane setting at
    // all — the stale open-time value would clobber the other client's change.
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setChangesPaneVisibility).not.toHaveBeenCalled()
  })

  it("Escape closes the dialog when no persist is in flight", () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    render(<CustomizeWebappDialog />)

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeCustomizeWebapp).toHaveBeenCalled()
  })

  it("ignores Escape while a persist is in flight, then closes when it settles", async () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    let resolveWrite!: (v: boolean) => void
    setInstanceIdentity.mockReturnValue(
      new Promise<boolean>((r) => {
        resolveWrite = r
      }),
    )
    render(<CustomizeWebappDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    // Escape (like backdrop clicks and the header X, which also route through
    // onOpenChange) must be inert while the write is pending.
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeCustomizeWebapp).not.toHaveBeenCalled()

    resolveWrite(true)
    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
  })

  it("a double-click on Save fires the persists once", async () => {
    seed({ title: "old instance", favicon: "", show_changes_pane: true })
    render(<CustomizeWebappDialog />)

    const saveButton = screen.getByRole("button", { name: "Save" })
    // Two synchronous clicks land before React re-renders the disabled state;
    // the in-flight ref must gate the second one.
    fireEvent.click(saveButton)
    fireEvent.click(saveButton)

    await waitFor(() => expect(closeCustomizeWebapp).toHaveBeenCalled())
    expect(setInstanceIdentity).toHaveBeenCalledTimes(1)
  })
})
