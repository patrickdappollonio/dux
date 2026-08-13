// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"
import type { ReactNode } from "react"
import type { PanelImperativeHandle, PanelProps } from "react-resizable-panels"

import type { DuxState } from "@/lib/store"

// What DesktopShell hands react-resizable-panels, and what it does with what
// the library hands back. The panes themselves are stubbed: TerminalArea drags
// in xterm and a live socket, and none of that is what these tests are about.
//
// The two bugs pinned here:
//
//   1. UNITS. v4 reads a BARE NUMBER as PIXELS (see the units note in
//      lib/editorLayout.ts). `minSize={14}` was a 14-PIXEL minimum, not 14%, so
//      the Changes panel could be dragged down to a sliver and, being
//      `collapsible`, snapped from there to zero.
//   2. A drag to zero wrote NOTHING. Visibility (the preference) and the split
//      (runtime) are unrelated variables, so the pane went to zero width while
//      the preference still said "visible": the header's reopen button never
//      appeared and the pane's own hide item was inside the zero-width pane.

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
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    })),
  )
}
installBootStubs()
const { DesktopShell } = await import("./App")
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

function panel(id: string): PanelProps | undefined {
  return recordedPanelProps.find((p) => p.id === id)
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

beforeEach(() => {
  recordedPanelProps.length = 0
  lastGroupProps = {}
  nextChangesHandle = null
  collapseFromDrag.mockClear()
  setPercent.mockClear()
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("DesktopShell panel units", () => {
  it("spells the unit out on every size it hands the panel library", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)

    const terminal = panel("terminal-pane")!
    const changes = panel("changes-pane")!
    // A bare number here means PIXELS: `minSize={14}` was a 14-pixel floor, so
    // the pane could be dragged to a sliver and then snapped to zero.
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
  it("hides the pane through the preference when a drag takes it to zero", () => {
    mockState = stateWith(true)
    render(<DesktopShell />)
    const onResize = panel("changes-pane")!.onResize!

    act(() => {
      onResize(
        { asPercentage: 0, inPixels: 0 },
        "changes-pane",
        { asPercentage: 26, inPixels: 400 },
      )
    })
    // Hidden-by-drag and hidden-by-menu are now ONE state, so the header's
    // reopen button appears. Without this the pane was unreachable.
    expect(collapseFromDrag).toHaveBeenCalledTimes(1)
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
  // the ZERO, not the default, and only a reload used to clear it.
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
    expect(handle.resized).toEqual([])
  })
})
