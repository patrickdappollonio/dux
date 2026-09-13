import { describe, expect, it } from "vitest"

import {
  agentIsDetachable,
  detachConfirmBody,
  forceStopConfirmBody,
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

describe("agentIsDetachable", () => {
  // The server's answer wins, because it is the same oracle the engine's own
  // teardown asks. A menu that offered a detach the server would refuse, or hid
  // one it would accept, is the drift this field exists to remove.
  it("takes the server's answer when it is there", () => {
    expect(
      agentIsDetachable({ detachable: true, tabs: [] }),
    ).toBe(true)
    expect(
      agentIsDetachable({
        detachable: false,
        tabs: [{ has_live_process: true }],
      }),
    ).toBe(false)
  })

  it("falls back to the tab scan against an older server", () => {
    expect(
      agentIsDetachable({
        tabs: [{ has_live_process: false }, { has_live_process: true }],
      }),
    ).toBe(true)
    expect(agentIsDetachable({ tabs: [{ has_live_process: false }] })).toBe(
      false,
    )
    expect(agentIsDetachable({ tabs: [] })).toBe(false)
  })
})

describe("forceStopConfirmBody", () => {
  it("promises no wait at all and says the agent survives as detached", () => {
    // The Task Manager's own words. It must not quote a grace period: there is
    // none, and the whole point of that surface is that it acts at once.
    expect(forceStopConfirmBody("fix-auth")).toBe(
      'dux will stop "fix-auth" immediately, with no shutdown wait. Anything ' +
        "it is doing right now is lost. The agent stays in the list as " +
        "Detached, and you can resume it later.",
    )
  })

  it("never quotes the grace the polite body is built around", () => {
    expect(forceStopConfirmBody("feat")).not.toContain("wait up to")
    expect(detachConfirmBody("feat", 30, 1)).toContain("wait up to 30 seconds")
  })
})
