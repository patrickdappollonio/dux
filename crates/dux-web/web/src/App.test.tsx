// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"
import type { ReactNode } from "react"
import type { PanelImperativeHandle, PanelProps } from "react-resizable-panels"

import type { DuxState } from "@/lib/store"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

// What DesktopShell hands react-resizable-panels, and what it does with what
// the library hands back. The panes themselves are stubbed: TerminalArea drags
// in xterm and a live socket, and none of that is what these tests are about.
//
// The two invariants pinned here:
//
//   1. UNITS. v4 reads a BARE NUMBER as PIXELS (see the units note in
//      lib/editorLayout.ts): a bare `minSize={14}` is a 14-pixel minimum, not
//      14%, letting the Changes panel be dragged to a sliver and, being
//      `collapsible`, snapped to zero.
//   2. A drag to zero must WRITE the preference. Visibility (the preference)
//      and the split (runtime) are unrelated variables, so an unwritten
//      collapse leaves the pane at zero width while the preference still says
//      "visible": no reopen button, and the pane's own hide item inside the
//      zero-width pane.

const recordedPanelProps: Array<PanelProps> = []
let lastGroupProps: Record<string, unknown> = {}
// What a mounting Changes panel reports its size as, so a test can stage the
// library's cached-zero. The stub publishes it into the shell's `panelRef` in a
// LAYOUT effect, which is when the real Panel's `useImperativeHandle` runs:
// before the parent's own effects, which is the ordering the re-show depends
// on.
let nextChangesHandle: FakeHandle | null = null
vi.mock("react-resizable-panels", async () => {
  const { useLayoutEffect } = await import("react")
  const Group = ({ children, ...rest }: { children: ReactNode }) => {
    lastGroupProps = rest as Record<string, unknown>
    return <div data-testid="panel-group">{children}</div>
  }
  const Panel = (props: PanelProps) => {
    recordedPanelProps.push(props)
    const ref = props.panelRef as
      | { current: PanelImperativeHandle | null }
      | undefined
    useLayoutEffect(() => {
      if (!ref || props.id !== "changes-pane" || !nextChangesHandle) return
      ref.current = nextChangesHandle
      return () => {
        ref.current = null
      }
    })
    return <div data-panel-id={String(props.id)}>{props.children}</div>
  }
  const Separator = () => <div data-testid="separator" />
  return { Group, Panel, Separator }
})

vi.mock("@/components/Sidebar", () => ({ AppSidebar: () => <div /> }))
vi.mock("@/components/InsetHeader", () => ({ InsetHeader: () => <div /> }))
vi.mock("@/components/TerminalArea", () => ({
  TerminalArea: () => <div data-testid="terminal-area" />,
}))
vi.mock("@/components/ChangedFiles", () => ({
  ChangedFiles: () => <div data-testid="changed-files" />,
}))

let mockState: DuxState
const collapseFromDrag = vi.fn()
const setPercent = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    collapseChangesPaneFromDrag: () => collapseFromDrag(),
    setChangesPanePercent: (p: number) => setPercent(p),
  }
})

let bootMedia: MatchMediaStub | null = null

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
  bootMedia?.restore()
  bootMedia = stubMatchMedia()
}
installBootStubs()
const { DesktopShell, CHANGES_PANE_HEAL_FRAMES } = await import("./App")
const { CHANGES_PANE_DEFAULT_PERCENT } = await import("@/lib/store")

function stateWith(showChanges: boolean, percent = 26): DuxState {
  return {
    sidebarWidth: "18rem",
    changesPanePercent: percent,
    changesPaneOverride: null,
    bootstrap: { show_changes_pane: showChanges },
    spine: { projects: [], sessions: [], terminals: [] },
    selectedTarget: null,
  } as unknown as DuxState
}

// jsdom has no PointerEvent constructor; the shell only reads the type, and it
// listens on `window` in the capture phase, so a bare Event dispatched there is
// exactly what it sees.
function pointer(
  type: "pointerdown" | "pointermove" | "pointerup" | "pointercancel",
  clientX = 0,
  clientY = 0,
): void {
  const event = new Event(type) as Event & { clientX: number; clientY: number }
  event.clientX = clientX
  event.clientY = clientY
  window.dispatchEvent(event)
}

// A gesture that actually moved. A press that goes nowhere decides nothing, so
// every test about a drag has to be a drag.
function dragPointer(from = 0, to = 200): void {
  pointer("pointerdown", from)
  pointer("pointermove", to)
}

function panel(id: string): PanelProps | undefined {
  return recordedPanelProps.find((p) => p.id === id)
}

// The props of the MOST RECENT mount of a panel, which is what a test about
// hiding and showing has to look at: `recordedPanelProps` accumulates across
// every render, so `panel()` above answers with the first mount forever.
function latestPanel(id: string): PanelProps | undefined {
  return recordedPanelProps.filter((p) => p.id === id).pop()
}

const SPLIT_KEY = "dux:changes-pane-percent"

function layout(changes: number): Record<string, number> {
  return { "changes-pane": changes, "terminal-pane": 100 - changes }
}

function reportLayout(changes: number): void {
  const onLayoutChange = lastGroupProps.onLayoutChange as (
    l: Record<string, number>,
  ) => void
  act(() => {
    onLayoutChange(layout(changes))
  })
}

// A stand-in for the library's imperative handle, so a test can say what the
// panel reports its width as and see what DesktopShell does about it.
type FakeHandle = PanelImperativeHandle & { resized: Array<number | string> }

function fakeHandle(asPercentage: number): FakeHandle {
  const resized: Array<number | string> = []
  return {
    collapse: () => {},
    expand: () => {},
    getSize: () => ({ asPercentage, inPixels: asPercentage * 10 }),
    isCollapsed: () => asPercentage < 1,
    resize: (size: number | string) => void resized.push(size),
    resized,
  }
}

// The real handle does not report a missing panel, it THROWS: every method
// resolves the panel through the library's registry and raises `Layout not
// found for Panel <id>` when the entry is not there. A re-mounting panel has no
// entry until the group re-registers, which is a frame away, so this is what
// the shell actually meets on the way back in.
function throwingHandle(
  throwsFor: number,
  thenPercentage: number,
): FakeHandle {
  const resized: Array<number | string> = []
  let calls = 0
  return {
    collapse: () => {},
    expand: () => {},
    getSize: () => {
      calls += 1
      if (calls <= throwsFor) {
        throw new Error("Layout not found for Panel changes-pane")
      }
      return { asPercentage: thenPercentage, inPixels: thenPercentage * 10 }
    },
    isCollapsed: () => thenPercentage < 1,
    resize: (size: number | string) => void resized.push(size),
    resized,
  }
}

// The heal runs off animation frames, and the drag-collapse write off a
// timeout, so the tests have to let both run. Real timers with a real rAF would
// make every one of these tests wait on the browser's frame cadence.
function flushFrames(count: number): void {
  for (let i = 0; i < count; i += 1) {
    act(() => {
      vi.advanceTimersByTime(17)
    })
  }
}

beforeEach(() => {
  recordedPanelProps.length = 0
  lastGroupProps = {}
  nextChangesHandle = null
  collapseFromDrag.mockClear()
  setPercent.mockClear()
  installBootStubs()
  vi.useFakeTimers()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  bootMedia?.restore()
  bootMedia = null
  vi.unstubAllGlobals()
})

// Run everything the shell has scheduled: the collapse write's timeout and the
// re-show heal's frames.
function flushScheduled(): void {
  act(() => {
    vi.runAllTimers()
  })
}

describe("DesktopShell panel units", () => {
  it("spells the unit out on every size it hands the panel library", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)

    const terminal = panel("terminal-pane")!
    const changes = panel("changes-pane")!
    // A bare number here means PIXELS: a bare `minSize={14}` is a 14-pixel
    // floor, so the pane can be dragged to a sliver and then snapped to zero.
    expect(terminal.minSize).toBe("30%")
    expect(terminal.defaultSize).toBe(`${100 - CHANGES_PANE_DEFAULT_PERCENT}%`)
    expect(changes.minSize).toBe("14%")
    expect(changes.defaultSize).toBe(`${CHANGES_PANE_DEFAULT_PERCENT}%`)
    for (const size of [
      terminal.minSize,
      terminal.defaultSize,
      changes.minSize,
      changes.defaultSize,
    ]) {
      expect(typeof size).toBe("string")
    }
  })

  it("gives the terminal pane the full width, in percent, when Changes is hidden", () => {
    mockState = stateWith(false)
    render(<DesktopShell />)
    expect(panel("terminal-pane")!.defaultSize).toBe("100%")
    expect(panel("changes-pane")).toBeUndefined()
  })
})

describe("DesktopShell drag-collapse", () => {
  it("hides the pane through the preference when the keyboard collapses it", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    // The library moves its separator from its own keydown handler and reports
    // the new layout inside that same dispatch. No pointer is involved, so the
    // travel rule cannot speak for it; the keystroke does.
    act(() => {
      window.dispatchEvent(new Event("keydown"))
      onResize(
        { asPercentage: 0, inPixels: 0 },
        "changes-pane",
        { asPercentage: 26, inPixels: 400 },
      )
    })
    // Never from inside the event that produced the report: the write unmounts
    // the panel, and the library is not done with it until its own listeners
    // have run. One task later is soon enough and is the whole guarantee.
    expect(collapseFromDrag).not.toHaveBeenCalled()
    flushScheduled()
    // Hidden-by-drag and hidden-by-menu are ONE state, so the header's
    // reopen button appears. Without this the pane is unreachable.
    expect(collapseFromDrag).toHaveBeenCalledTimes(1)
  })

  it("waits for the pointer to come up before writing a dragged collapse", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => dragPointer())
    act(() => {
      onResize({ asPercentage: 0.5, inPixels: 8 }, "changes-pane", {
        asPercentage: 26,
        inPixels: 400,
      })
    })
    // Writing here would flip the preference and unmount the panel and its
    // separator MID-GESTURE. react-resizable-panels 4.11.2 then re-registers
    // the pre-unmount group on its own pointerup, leaving a detached separator
    // that hit-tests as a phantom zone at the viewport's corner and leaking a
    // registry entry per collapse.
    expect(collapseFromDrag).not.toHaveBeenCalled()

    // And not synchronously on the release either. This shell listens on the
    // WINDOW in the capture phase; the library listens on the DOCUMENT in the
    // capture phase, which is strictly later in the same dispatch, so writing
    // here still unmounts the panel while the library is mid-drag. Ending the
    // drag then re-adds the group object it captured at pointerdown to the
    // registry, resurrecting the registration that just died, and every lookup
    // scans by id and takes the first match. Measured: the reopened pane is an
    // eleven-pixel sliver, its layout never reappears, and the library throws
    // `Invalid 2 panel layout: 100%` from its own
    // ResizeObserver. A macrotask lands after the whole dispatch; a microtask
    // would not, because microtask checkpoints run between listeners.
    act(() => pointer("pointerup"))
    expect(collapseFromDrag).not.toHaveBeenCalled()
    flushScheduled()
    expect(collapseFromDrag).toHaveBeenCalledTimes(1)
  })

  it("keeps the pane when the drag comes back out before release", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => dragPointer())
    act(() => {
      onResize({ asPercentage: 0.5, inPixels: 8 }, "changes-pane", {
        asPercentage: 26,
        inPixels: 400,
      })
      onResize({ asPercentage: 20, inPixels: 320 }, "changes-pane", {
        asPercentage: 0.5,
        inPixels: 8,
      })
    })
    act(() => pointer("pointerup"))
    flushScheduled()
    // Dragging past the snap and back out before letting go is the escape from
    // an accidental collapse; committing at the snap takes it away.
    expect(collapseFromDrag).not.toHaveBeenCalled()
  })

  // THE TABLET BUG. Measured on a 1180x820 touch viewport: a press within the
  // divider's grab band but on the changed-files list beside it was still
  // acquired by react-resizable-panels (it hit-tests a rectangle on the
  // document, not the element), the browser then claimed the same touch as a
  // scroll and sent `pointercancel`, and the `pointerleave` that followed was
  // handled by the library as a move with no press point, which its own code
  // reads as a full-scale delta. The pane went to zero and the shell wrote
  // `show_changes_pane: false` to the server. A tap closed the Changes pane.
  it("commits nothing when the browser cancels the gesture", () => {
    mockState = stateWith(true)
    nextChangesHandle = fakeHandle(26)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => {
      pointer("pointerdown", 300)
      pointer("pointercancel", 300)
      // What the library reports out of the `pointerleave` that follows.
      onResize({ asPercentage: 0, inPixels: 0 }, "changes-pane", {
        asPercentage: 26,
        inPixels: 400,
      })
    })
    flushScheduled()

    // No preference write, so the pane is still shown.
    expect(collapseFromDrag).not.toHaveBeenCalled()
    // And nothing remembered either: a cancelled gesture is not a decision.
    expect(localStorage.getItem(SPLIT_KEY)).toBeNull()
    // Refusing the write alone would leave a zero-width pane whose preference
    // still says visible, which is the original stranding bug. The split goes
    // back to where it was.
    expect(nextChangesHandle!.resized).toEqual(["26%"])
  })

  it("restores a late zero report after a moved gesture is cancelled", () => {
    mockState = stateWith(true)
    nextChangesHandle = fakeHandle(26)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => {
      pointer("pointerdown", 300)
      pointer("pointermove", 340)
      pointer("pointercancel", 340)
    })
    // react-resizable-panels can publish this after pointercancel, when its
    // stale pointerleave handling drives the pane to zero.
    act(() => {
      onResize({ asPercentage: 0, inPixels: 0 }, "changes-pane", {
        asPercentage: 26,
        inPixels: 400,
      })
    })
    flushScheduled()

    expect(collapseFromDrag).not.toHaveBeenCalled()
    expect(localStorage.getItem(SPLIT_KEY)).toBeNull()
    expect(nextChangesHandle!.resized).toEqual(["26%"])
  })

  it("commits nothing when a press never travelled", () => {
    mockState = stateWith(true)
    nextChangesHandle = fakeHandle(26)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => {
      // Down and straight back up, a pixel of jitter apart: a tap.
      pointer("pointerdown", 300)
      pointer("pointermove", 301)
      pointer("pointerup", 301)
      onResize({ asPercentage: 0, inPixels: 0 }, "changes-pane", {
        asPercentage: 26,
        inPixels: 400,
      })
    })
    flushScheduled()

    expect(collapseFromDrag).not.toHaveBeenCalled()
    expect(nextChangesHandle!.resized).toEqual(["26%"])
  })

  it("says nothing about an ordinary resize, or about a panel's first report", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => {
      onResize(
        { asPercentage: 18, inPixels: 300 },
        "changes-pane",
        { asPercentage: 26, inPixels: 400 },
      )
      // The first report of the panel's life: no previous size. Treating this
      // as a collapse would hide the pane during its own mount.
      onResize({ asPercentage: 0, inPixels: 0 }, "changes-pane", undefined)
    })
    flushScheduled()
    expect(collapseFromDrag).not.toHaveBeenCalled()
  })

  it("mirrors the live split into the store on every layout report", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)
    const onLayoutChange = lastGroupProps.onLayoutChange as (l: {
      [id: string]: number
    }) => void
    act(() => onLayoutChange({ "changes-pane": 33, "terminal-pane": 67 }))
    expect(setPercent).toHaveBeenCalledWith(33)
  })
})

describe("DesktopShell re-show", () => {
  // The library caches the group's layout by joined panel ids and prefers that
  // cache over `defaultSize` when the panels re-register (measured in
  // react-resizable-panels 4.11.2: `mutableState.layouts[ids] ?? defaultLayout
  // ?? computed`). So re-showing a pane that was collapsed to zero brings back
  // the ZERO, not the default, and only a reload clears that cache.
  it("resets a re-shown pane that comes back at nothing", () => {
    // Hidden, then shown again with the panel reporting the cached zero.
    mockState = stateWith(false)
    const { rerender } = render(<DesktopShell />)
    const handle = fakeHandle(0)
    nextChangesHandle = handle
    mockState = stateWith(true)
    act(() => {
      rerender(<DesktopShell />)
    })
    flushFrames(1)

    expect(handle.resized).toEqual([`${CHANGES_PANE_DEFAULT_PERCENT}%`])
    // The header's spacer mirrors the store's number, so it has to move too.
    expect(setPercent).toHaveBeenCalledWith(CHANGES_PANE_DEFAULT_PERCENT)
  })

  it("leaves a re-shown pane alone when it comes back at a real width", () => {
    mockState = stateWith(false)
    const { rerender } = render(<DesktopShell />)
    const handle = fakeHandle(31)
    nextChangesHandle = handle
    mockState = stateWith(true)
    act(() => {
      rerender(<DesktopShell />)
    })
    flushFrames(CHANGES_PANE_HEAL_FRAMES + 1)

    expect(handle.resized).toEqual([])
  })

  it("does not resize the pane on a first mount", () => {
    // No hidden→shown transition, so nothing to heal: the panel is at its
    // defaultSize and the library has no cached layout to prefer over it.
    const handle = fakeHandle(0)
    nextChangesHandle = handle
    mockState = stateWith(true)
    act(() => {
      render(<DesktopShell />)
    })
    flushFrames(CHANGES_PANE_HEAL_FRAMES + 1)
    expect(handle.resized).toEqual([])
  })

  // The handle's methods throw rather than no-op while the re-mounting panel
  // has no registry entry, so a `getSize()` straight from the shell's own
  // effect is one beat too early: the throw unwinds React with no boundary over
  // it and blacks the whole screen on the first click of "Show Changes pane".
  it("survives a panel that has no layout yet, and heals once it does", () => {
    mockState = stateWith(false)
    const { rerender } = render(<DesktopShell />)
    const handle = throwingHandle(1, 0)
    nextChangesHandle = handle
    mockState = stateWith(true)
    expect(() => {
      act(() => {
        rerender(<DesktopShell />)
      })
    }).not.toThrow()
    // Nothing yet: the first look still throws.
    flushFrames(1)
    expect(handle.resized).toEqual([])
    // The layout lands, and the pane is healed to its default width.
    flushFrames(1)
    expect(handle.resized).toEqual([`${CHANGES_PANE_DEFAULT_PERCENT}%`])
  })

  it("gives up with a breadcrumb, not a crash, when the layout never lands", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    mockState = stateWith(false)
    const { rerender } = render(<DesktopShell />)
    // Throws for longer than the shell is willing to wait.
    const handle = throwingHandle(CHANGES_PANE_HEAL_FRAMES + 5, 0)
    nextChangesHandle = handle
    mockState = stateWith(true)
    act(() => {
      rerender(<DesktopShell />)
    })
    expect(() => flushFrames(CHANGES_PANE_HEAL_FRAMES + 3)).not.toThrow()
    expect(handle.resized).toEqual([])
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
  })

  it("does not crash, or leave the pane hidden, when the resize itself throws", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    mockState = stateWith(false)
    const { rerender } = render(<DesktopShell />)
    const handle = fakeHandle(0)
    handle.resize = () => {
      throw new Error("Layout not found for Panel changes-pane")
    }
    nextChangesHandle = handle
    mockState = stateWith(true)
    act(() => {
      rerender(<DesktopShell />)
    })
    expect(() => flushFrames(2)).not.toThrow()
    // The spacer must NOT move to a width the panel refused to take.
    expect(setPercent).not.toHaveBeenCalledWith(CHANGES_PANE_DEFAULT_PERCENT)
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
  })

  // A pane coming back from hidden re-mounts into the library's cached layout
  // and reports that width like any other resize. Reading a cached zero as a
  // drag-collapse would hide the pane during the act of showing it, so the
  // user's click would look like it did nothing at all.
  it("does not read the mount width of a re-shown pane as a collapse", () => {
    mockState = stateWith(false)
    const { rerender } = render(<DesktopShell />)
    // Still no layout, so the heal window stays open across these reports.
    const handle = throwingHandle(2, 0)
    nextChangesHandle = handle
    mockState = stateWith(true)
    act(() => {
      rerender(<DesktopShell />)
    })
    const onResize = panel("changes-pane")!.onResize!
    act(() => {
      // The mount settling: a real width, then the cached zero. Off a re-show
      // that pair is not a gesture, and there is no pointer down to wait for.
      onResize({ asPercentage: 26, inPixels: 400 }, "changes-pane", undefined)
      onResize({ asPercentage: 0, inPixels: 0 }, "changes-pane", {
        asPercentage: 26,
        inPixels: 400,
      })
    })
    flushScheduled()
    expect(collapseFromDrag).not.toHaveBeenCalled()
  })
})


// THE REMEMBERED SPLIT. The panel publishes a layout for reasons that are not
// the user moving the divider: its own mount, its unmount, and the re-show
// heal. Writing every report handed the remembered width back to itself, so a
// pane dragged to 40 came home from one hide-and-show as the mount default.
describe("DesktopShell remembered split", () => {
  it("writes a released drag, and brings it back through a hide and a show", () => {
    mockState = stateWith(true)
    const { rerender } = render(<DesktopShell />)
    expect(panel("changes-pane")!.defaultSize).toBe(
      `${CHANGES_PANE_DEFAULT_PERCENT}%`,
    )

    // A drag: pointer down, the panel reports as it moves, pointer up.
    act(() => {
      dragPointer()
    })
    reportLayout(40)
    act(() => {
      pointer("pointerup")
    })
    expect(localStorage.getItem(SPLIT_KEY)).toBe("40")

    // Hide it, show it again: it comes back at 40, not at the default.
    mockState = stateWith(false)
    act(() => {
      rerender(<DesktopShell />)
    })
    mockState = stateWith(true)
    act(() => {
      rerender(<DesktopShell />)
    })
    expect(latestPanel("changes-pane")!.defaultSize).toBe("40%")

    // And the mount's own layout report leaves the memory alone.
    reportLayout(40)
    expect(localStorage.getItem(SPLIT_KEY)).toBe("40")
  })

  it("ignores a layout report with nothing held down", () => {
    localStorage.setItem(SPLIT_KEY, "40")
    mockState = stateWith(true)
    render(<DesktopShell />)
    expect(panel("changes-pane")!.defaultSize).toBe("40%")

    // What a mount, an unmount or the heal publishes. No gesture, no write.
    reportLayout(CHANGES_PANE_DEFAULT_PERCENT)
    expect(localStorage.getItem(SPLIT_KEY)).toBe("40")
  })

  it("writes a keyboard step, which the library makes inside the keydown", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)

    act(() => {
      window.dispatchEvent(new Event("keydown"))
      const onLayoutChange = lastGroupProps.onLayoutChange as (
        l: Record<string, number>,
      ) => void
      onLayoutChange(layout(33))
    })
    expect(localStorage.getItem(SPLIT_KEY)).toBe("33")
  })

  it("stops believing the keyboard once the keystroke is over", async () => {
    mockState = stateWith(true)
    render(<DesktopShell />)

    act(() => {
      window.dispatchEvent(new Event("keydown"))
    })
    // The flag is cleared in a microtask, so anything reported in a later task
    // is a mount, not that keystroke.
    await act(async () => {})
    reportLayout(44)
    expect(localStorage.getItem(SPLIT_KEY)).toBeNull()
  })

  it("refuses a split that would squeeze the terminal under its own floor", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)

    act(() => {
      dragPointer()
    })
    reportLayout(85)
    act(() => {
      pointer("pointerup")
    })
    expect(localStorage.getItem(SPLIT_KEY)).toBeNull()
  })

  it("does not remember a drag that shut the pane", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)

    act(() => {
      dragPointer()
    })
    reportLayout(0)
    act(() => {
      pointer("pointerup")
    })
    flushScheduled()
    // The collapse rides the visibility preference, not the remembered split:
    // a stored zero would reopen the pane at nothing.
    expect(localStorage.getItem(SPLIT_KEY)).toBeNull()
  })
})
