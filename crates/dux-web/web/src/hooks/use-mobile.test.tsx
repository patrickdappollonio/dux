// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { isMobileViewport, useIsMobile } from "@/hooks/use-mobile"
import { setLayoutChoice } from "@/lib/shellLayout"
import { stubCoarsePointer } from "@/test/matchMedia"

function Probe() {
  return <span data-testid="layout">{useIsMobile() ? "phone" : "computer"}</span>
}

function landscapePhone() {
  vi.stubGlobal("innerWidth", 851)
  vi.stubGlobal("screen", { width: 851, height: 393 })
}

describe("useIsMobile", () => {
  afterEach(() => {
    cleanup()
    setLayoutChoice(null)
    localStorage.clear()
    vi.unstubAllGlobals()
  })

  it("gives a phone turned sideways the phone layout", () => {
    const media = stubCoarsePointer(true)
    try {
      landscapePhone()
      const view = render(<Probe />)
      expect(view.getByTestId("layout").textContent).toBe("phone")
      expect(isMobileViewport()).toBe(true)
    } finally {
      media.restore()
    }
  })

  it("follows the person's choice and keeps it across a remount", () => {
    const media = stubCoarsePointer(true)
    try {
      landscapePhone()
      const view = render(<Probe />)
      act(() => setLayoutChoice("computer"))
      expect(view.getByTestId("layout").textContent).toBe("computer")
      view.unmount()
      const again = render(<Probe />)
      expect(again.getByTestId("layout").textContent).toBe("computer")
      expect(isMobileViewport()).toBe(false)
    } finally {
      media.restore()
    }
  })

  it("moves with a rotation while nothing is chosen", () => {
    const media = stubCoarsePointer(true)
    try {
      vi.stubGlobal("screen", { width: 820, height: 1180 })
      vi.stubGlobal("innerWidth", 820)
      const view = render(<Probe />)
      expect(view.getByTestId("layout").textContent).toBe("computer")
      act(() => {
        vi.stubGlobal("innerWidth", 600)
        window.dispatchEvent(new Event("resize"))
      })
      expect(view.getByTestId("layout").textContent).toBe("phone")
    } finally {
      media.restore()
    }
  })
})
