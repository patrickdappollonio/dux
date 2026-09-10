// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

// Monaco itself cannot run in jsdom, and the bootstrap module pulls the real
// editor in on import. The stub below stands in for the widget and hands
// `onMount` a fake shaped like the one narrow read this component makes.
vi.mock("@/lib/monacoSetup", () => ({
  monacoLanguageForPath: () => "plaintext",
  monaco: {},
}))

let quitEarlyAnswer: unknown = { quitEarly: false }
let fireDiffUpdate: () => void = () => {}
let disposed = false

vi.mock("@monaco-editor/react", () => ({
  DiffEditor: ({
    onMount,
    options,
  }: {
    onMount: (editor: unknown) => void
    options: Record<string, unknown>
  }) => {
    const editor = {
      getModel: () => null,
      onDidUpdateDiff: (listener: () => void) => {
        fireDiffUpdate = listener
        return {
          dispose: () => {
            disposed = true
          },
        }
      },
      getDiffComputationResult: () => quitEarlyAnswer,
    }
    onMount(editor)
    return (
      <div
        data-testid="diff-editor"
        data-max-computation-time={String(options.maxComputationTime)}
      />
    )
  },
}))

const { default: DiffViewer } = await import("./DiffViewer")
const { diffQuitEarly } = await import("@/lib/diffComputation")

afterEach(() => {
  cleanup()
  quitEarlyAnswer = { quitEarly: false }
  disposed = false
})

describe("diffQuitEarly", () => {
  it("reads the flag off the widget's computation result", () => {
    expect(diffQuitEarly({ getDiffComputationResult: () => ({ quitEarly: true }) })).toBe(true)
    expect(diffQuitEarly({ getDiffComputationResult: () => ({ quitEarly: false }) })).toBe(false)
  })

  it("treats an answer it cannot read as finished", () => {
    // The method is absent from Monaco's public types, so a version that drops
    // it must leave the viewer quiet rather than claiming the diff was cut.
    expect(diffQuitEarly({})).toBe(false)
    expect(diffQuitEarly(null)).toBe(false)
    expect(diffQuitEarly({ getDiffComputationResult: () => null })).toBe(false)
  })
})

describe("DiffViewer", () => {
  it("says nothing while the diff computation finishes", () => {
    render(<DiffViewer path="a.txt" original="a\n" modified="b\n" />)
    act(() => fireDiffUpdate())
    expect(screen.queryByTestId("diff-quit-early-notice")).toBeNull()
  })

  it("says so when Monaco gave up on the diff", () => {
    quitEarlyAnswer = { quitEarly: true }
    render(<DiffViewer path="a.txt" original="a\n" modified="b\n" />)
    act(() => fireDiffUpdate())
    expect(screen.getByTestId("diff-quit-early-notice").textContent).toBe(
      "This diff was cut short because it took too long to compute; the highlights may be coarse.",
    )
  })

  it("gives the computation six times Monaco's default budget", () => {
    render(<DiffViewer path="a.txt" original="a\n" modified="b\n" />)
    expect(
      screen.getByTestId("diff-editor").getAttribute("data-max-computation-time"),
    ).toBe("30000")
  })

  it("drops its diff listener on unmount", () => {
    const view = render(<DiffViewer path="a.txt" original="a\n" modified="b\n" />)
    view.unmount()
    expect(disposed).toBe(true)
  })
})
