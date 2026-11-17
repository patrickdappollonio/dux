// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { ImageZoomViewer } from "@/components/ImageZoomViewer"

function mountLoaded() {
  render(<ImageZoomViewer src="/raw/shot.png" alt="shot.png" raster />)
  const img = screen.getByAltText("shot.png") as HTMLImageElement
  // jsdom never decodes images; stamp a natural size and fire load.
  Object.defineProperty(img, "naturalWidth", { value: 400 })
  Object.defineProperty(img, "naturalHeight", { value: 6000 })
  fireEvent.load(img)
  return img
}

const pressed = (name: RegExp) =>
  screen.getByRole("button", { name }).getAttribute("aria-pressed")

describe("ImageZoomViewer", () => {
  afterEach(cleanup)

  it("opens in Fit and the mode buttons switch the mode", () => {
    mountLoaded()
    expect(pressed(/^fit$/i)).toBe("true")
    fireEvent.click(screen.getByRole("button", { name: /fit width/i }))
    expect(pressed(/fit width/i)).toBe("true")
    expect(pressed(/^fit$/i)).toBe("false")
    fireEvent.click(screen.getByRole("button", { name: /actual size/i }))
    expect(pressed(/actual size/i)).toBe("true")
    expect(screen.getByTestId("image-zoom-level").textContent).toBe("100%")
    fireEvent.click(screen.getByRole("button", { name: /zoom in/i }))
    expect(screen.getByTestId("image-zoom-level").textContent).toBe("125%")
    expect(pressed(/actual size/i)).toBe("false")
  })

  it("a double click toggles between Fit and 100%", () => {
    const img = mountLoaded()
    fireEvent.doubleClick(img)
    expect(pressed(/actual size/i)).toBe("true")
    fireEvent.doubleClick(img)
    expect(pressed(/^fit$/i)).toBe("true")
  })
})
