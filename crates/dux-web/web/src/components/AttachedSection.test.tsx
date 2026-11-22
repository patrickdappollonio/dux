// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { AttachedBlocker } from "@/lib/attached"

vi.mock("@/lib/store", () => ({ useDux: () => ({ spine: null }) }))

const { AttachedSection } = await import("./AttachedSection")
const { guardedConfirmAction } = await import("./guardedConfirmAction")

function blocker(key: string, driving: boolean): AttachedBlocker {
  return {
    surface: "browser",
    device: "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0",
    address: "10.0.0.7",
    verified: true,
    driving,
    target: { kind: "tab", id: `${key}-slot`, agent: key },
    key,
  }
}

afterEach(cleanup)

describe("a guarded dialog told who is in the way", () => {
  it("offers no way past a device that is driving, and says to take over first", () => {
    const blockers = [blocker("a", true), blocker("b", false)]
    render(<AttachedSection blockers={blockers} />)
    expect(screen.getByText(/only the device driving it can change it\. Take it over/)).toBeTruthy()
    expect(screen.getByText(/typing in tab/)).toBeTruthy()
    expect(screen.getByText(/watching tab/)).toBeTruthy()
    const action = guardedConfirmAction({ verb: "Remove", blockers, pending: false, onConfirm: () => {} })
    expect(action.label).toBe("Remove")
    expect(action.disabledReason).not.toBeNull()
  })

  it("offers to go ahead over devices that only have it open", () => {
    const blockers = [blocker("b", false)]
    render(<AttachedSection blockers={blockers} />)
    expect(screen.queryByText(/Take it over/)).toBeNull()
    const action = guardedConfirmAction({ verb: "Remove", blockers, pending: false, onConfirm: () => {} })
    expect(action.label).toBe("Remove anyway")
    expect(action.disabledReason).toBeNull()
  })

  it("names every device that drives, and says so of each row it would change", () => {
    const base = { verb: "Remove", pending: false, onConfirm: () => {} }
    const tui: AttachedBlocker = {
      ...blocker("b", true),
      surface: "terminal_ui",
      device: null,
      target: { kind: "terminal", id: "term-3" },
    }
    expect(
      guardedConfirmAction({ ...base, blockers: [blocker("a", true), tui] }).disabledReason,
    ).toBe(
      "Firefox on Linux and the dux TUI are driving agents or terminals this would change. Take them over on this device first.",
    )
    expect(
      guardedConfirmAction({ ...base, blockers: [tui] }).disabledReason,
    ).toBe("The dux TUI is driving this right now. Take it over on this device first.")
  })
})
