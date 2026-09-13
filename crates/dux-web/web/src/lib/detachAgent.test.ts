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
  // The exact sentences, pinned verbatim. The Rust half asserts these same two
  // strings (`the_confirm_body_reads_the_same_on_both_surfaces` in
  // crates/dux-core/src/engine/lifecycle.rs), so a change to either surface's
  // copy fails on the side that changed instead of quietly leaving the two
  // dialogs disagreeing about the same act.
  it("reads exactly as the terminal UI's does, for one running tab", () => {
    expect(detachConfirmBody("feat/login", 30, 1)).toBe(
      'dux will ask "feat/login" to shut down and wait up to 30 seconds for it ' +
        "to exit before forcing it. The agent stays in the list as Detached, and " +
        "you can resume it later. Anything the agent is doing right now is " +
        "interrupted.",
    )
  })

  it("reads exactly as the terminal UI's does, for three running tabs", () => {
    expect(detachConfirmBody("feat/login", 45, 3)).toBe(
      'dux will ask "feat/login" to shut down and wait up to 45 seconds for it ' +
        "to exit before forcing it. The agent stays in the list as Detached, and " +
        "you can resume it later. Anything the agent is doing right now is " +
        "interrupted. All 3 running tabs stop together.",
    )
  })

  it("adds no tail below two running tabs", () => {
    expect(detachConfirmBody("a", 30, 0)).not.toContain("stop together")
    expect(detachConfirmBody("a", 30, 1)).not.toContain("stop together")
  })

  it("never hardcodes the default wait", () => {
    expect(detachConfirmBody("a", 7, 1)).toContain("7 seconds")
    expect(detachConfirmBody("a", 7, 1)).not.toContain("30 seconds")
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
