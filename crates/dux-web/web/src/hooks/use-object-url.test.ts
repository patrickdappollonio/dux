// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { renderHook } from "@testing-library/react"

import { useObjectUrl } from "./use-object-url"

// The SVG preview renders the CURRENT DRAFT through a Blob object URL. Object
// URLs are manual-lifetime: every create must be paired with a revoke or the
// blob leaks for the life of the page. This suite pins the whole lifecycle:
// one URL per content value, the previous URL revoked on change, the last one
// revoked on unmount, and nothing created for null content.

let counter = 0
const created: Blob[] = []
const createMock = vi.fn((blob: Blob) => {
  created.push(blob)
  counter += 1
  return `blob:test-${counter}`
})
const revokeMock = vi.fn()

beforeEach(() => {
  counter = 0
  created.length = 0
  createMock.mockClear()
  revokeMock.mockClear()
  // jsdom has no createObjectURL; stub the pair the hook must call.
  vi.stubGlobal("URL", {
    ...URL,
    createObjectURL: createMock,
    revokeObjectURL: revokeMock,
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("useObjectUrl", () => {
  it("returns null and creates nothing for null content", () => {
    const { result, unmount } = renderHook(() =>
      useObjectUrl(null, "image/svg+xml"),
    )
    expect(result.current).toBe(null)
    expect(createMock).not.toHaveBeenCalled()
    unmount()
    expect(revokeMock).not.toHaveBeenCalled()
  })

  it("creates one URL per content value with the given MIME type", () => {
    const { result } = renderHook(
      ({ content }) => useObjectUrl(content, "image/svg+xml"),
      { initialProps: { content: "<svg/>" } },
    )
    expect(result.current).toBe("blob:test-1")
    expect(createMock).toHaveBeenCalledTimes(1)
    expect(created[0].type).toBe("image/svg+xml")
  })

  it("a re-render with the SAME content reuses the URL", () => {
    const { result, rerender } = renderHook(
      ({ content }) => useObjectUrl(content, "image/svg+xml"),
      { initialProps: { content: "<svg/>" } },
    )
    rerender({ content: "<svg/>" })
    expect(result.current).toBe("blob:test-1")
    expect(createMock).toHaveBeenCalledTimes(1)
    expect(revokeMock).not.toHaveBeenCalled()
  })

  it("a content change mints a new URL and revokes the previous one", () => {
    const { result, rerender } = renderHook(
      ({ content }) => useObjectUrl(content, "image/svg+xml"),
      { initialProps: { content: "<svg/>" } },
    )
    rerender({ content: "<svg><g/></svg>" })
    expect(result.current).toBe("blob:test-2")
    expect(revokeMock).toHaveBeenCalledWith("blob:test-1")
  })

  it("content going null revokes and returns null", () => {
    const { result, rerender } = renderHook(
      ({ content }: { content: string | null }) =>
        useObjectUrl(content, "image/svg+xml"),
      { initialProps: { content: "<svg/>" as string | null } },
    )
    rerender({ content: null })
    expect(result.current).toBe(null)
    expect(revokeMock).toHaveBeenCalledWith("blob:test-1")
  })

  it("unmount revokes the live URL", () => {
    const { unmount } = renderHook(() => useObjectUrl("<svg/>", "image/svg+xml"))
    unmount()
    expect(revokeMock).toHaveBeenCalledWith("blob:test-1")
  })
})
