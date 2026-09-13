import { describe, expect, it } from "vitest"

import {
  agentHasLiveProcess,
  detachConfirmBody,
  shutdownGraceSeconds,
} from "./detachAgent"

describe("shutdownGraceSeconds", () => {
  it("uses the number the server projected", () => {
    expect(shutdownGraceSeconds({ shutdown_timeout_seconds: 45 })).toBe(45)
  })

  it("keeps a configured zero, which means force it at once", () => {
    expect(shutdownGraceSeconds({ shutdown_timeout_seconds: 0 })).toBe(0)
  })

  it("falls back to the documented default before the document lands", () => {
    expect(shutdownGraceSeconds(null)).toBe(30)
    expect(shutdownGraceSeconds({})).toBe(30)
  })
})

describe("detachConfirmBody", () => {
  it("quotes the agent and the wait, and says the agent stays reopenable", () => {
    const body = detachConfirmBody("feat/login", 45)
    expect(body).toContain('"feat/login"')
    expect(body).toContain("wait up to 45 seconds")
    expect(body).toContain("stays in the list as Detached")
    expect(body).toContain("resume it later")
    expect(body).toContain("interrupted")
  })

  it("never hardcodes the default wait", () => {
    expect(detachConfirmBody("a", 7)).toContain("7 seconds")
    expect(detachConfirmBody("a", 7)).not.toContain("30 seconds")
  })
})

describe("agentHasLiveProcess", () => {
  it("is true when any tab is running, not only the first", () => {
    expect(
      agentHasLiveProcess({
        tabs: [{ has_live_process: false }, { has_live_process: true }],
      }),
    ).toBe(true)
  })

  it("is false for a dormant agent and for one with no tabs at all", () => {
    expect(agentHasLiveProcess({ tabs: [{ has_live_process: false }] })).toBe(
      false,
    )
    expect(agentHasLiveProcess({ tabs: [] })).toBe(false)
  })
})
