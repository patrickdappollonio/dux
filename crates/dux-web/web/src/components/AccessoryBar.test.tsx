// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { AccessoryBar } from "./AccessoryBar"

// The accessory keys' activation contract, the same one the compose bar's Send
// button carries: act on pointerdown WITH preventDefault (so a tap never moves
// focus — the soft keyboard state is preserved, whichever it was), and accept
// keyboard/AT activation through a `detail === 0` click. A click that follows
// a real pointer tap carries `detail >= 1` and must be ignored, or every tap
// would fire the key twice.

function renderBar(overrides: Partial<Parameters<typeof AccessoryBar>[0]> = {}) {
  const props = {
    onEsc: vi.fn(),
    onTab: vi.fn(),
    onNewline: vi.fn(),
    onArrow: vi.fn(),
    onScroll: vi.fn(),
    ctrl: false,
    alt: false,
    onToggleCtrl: vi.fn(),
    onToggleAlt: vi.fn(),
    ...overrides,
  }
  render(<AccessoryBar {...props} />)
  return props
}

afterEach(cleanup)

describe("AccessoryBar key activation", () => {
  it("fires on pointerdown and preventDefaults so the press never takes focus", () => {
    const props = renderBar()
    // fireEvent returns false when a handler called preventDefault: the
    // suppressed default is what keeps focus (and the soft keyboard) where
    // they were before the tap.
    expect(
      fireEvent.pointerDown(screen.getByRole("button", { name: "Esc" })),
    ).toBe(false)
    expect(props.onEsc).toHaveBeenCalledTimes(1)
  })

  it("a keyboard/AT activation (click with detail 0) fires the key", () => {
    const props = renderBar()
    fireEvent.click(screen.getByRole("button", { name: "Tab" }), { detail: 0 })
    expect(props.onTab).toHaveBeenCalledTimes(1)
  })

  it("the click that follows a pointer tap (detail 1) does not double-fire", () => {
    const props = renderBar()
    const esc = screen.getByRole("button", { name: "Esc" })
    fireEvent.pointerDown(esc)
    fireEvent.click(esc, { detail: 1 })
    expect(props.onEsc).toHaveBeenCalledTimes(1)
  })

  it("every key row honors the same contract (arrows and page scroll included)", () => {
    const props = renderBar()
    fireEvent.click(screen.getByRole("button", { name: "Left" }), {
      detail: 0,
    })
    expect(props.onArrow).toHaveBeenCalledWith("left")
    fireEvent.click(screen.getByRole("button", { name: "Page down" }), {
      detail: 0,
    })
    expect(props.onScroll).toHaveBeenCalledWith("pageDown")
    fireEvent.click(screen.getByRole("button", { name: "Insert newline" }), {
      detail: 0,
    })
    expect(props.onNewline).toHaveBeenCalledTimes(1)
  })
})

// THE KEY ROW IS TERMINAL KEYS ONLY. It carried a "Box"/"Direct" cap that
// changed the typing surface, one cell wide among keys that merely type, where
// a thumb reaching for the newline could hit it. The action lives in the input
// `⋯` beside it, which is a menu and says what it does.
describe("AccessoryBar surface controls", () => {
  it("carries no typing-surface control of any kind", () => {
    renderBar()
    expect(screen.queryByRole("button", { name: /^Typing surface:/ })).toBeNull()
    expect(screen.queryByText("Box")).toBeNull()
    expect(screen.queryByText("Direct")).toBeNull()
  })
})
