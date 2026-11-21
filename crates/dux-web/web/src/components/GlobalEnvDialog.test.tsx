// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, closeGlobalEnv: vi.fn() }
})

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

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
const { GlobalEnvDialog } = await import("./GlobalEnvDialog")

function seed() {
  mockState = {
    bootstrap: { global_env: { FOO: "1" } },
    globalEnvOpen: true,
    globalEnvDraft: null,
    globalEnvConflict: null,
    globalEnvEpoch: 0,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  seed()
})
afterEach(() => {
  cleanup()
  phone = false
})

describe("GlobalEnvDialog", () => {
  it("puts the caret in the text on a computer", () => {
    render(<GlobalEnvDialog />)
    expect(document.activeElement).toBe(screen.getByPlaceholderText("KEY=VALUE"))
  })

  it("does not focus the text on a phone, where that raises the keyboard", () => {
    phone = true
    render(<GlobalEnvDialog />)
    expect(document.querySelector('[data-slot="sheet-content"]')?.contains(
      screen.getByPlaceholderText("KEY=VALUE"),
    )).toBe(true)
    expect(document.activeElement).not.toBe(screen.getByPlaceholderText("KEY=VALUE"))
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy()
  })
})
