// @vitest-environment jsdom
import { describe, expect, it } from "vitest"

import {
  LIFECYCLE_EVENTS,
  applyLifecycle,
  lifecycleAction,
  registerPageLifecycle,
  type LifecycleParticipant,
} from "./pageLifecycle"

function spy(): LifecycleParticipant & { calls: string[] } {
  const calls: string[] = []
  return {
    calls,
    close: () => calls.push("close"),
    resumeNow: () => calls.push("resumeNow"),
    park: () => calls.push("park"),
  }
}

describe("the lifecycle table", () => {
  it("maps each event to the action the socket owes it", () => {
    expect(lifecycleAction("pagehide")).toBe("close")
    expect(lifecycleAction("pageshow")).toBe("reopen")
    expect(lifecycleAction("freeze")).toBe("park")
    expect(lifecycleAction("resume")).toBe("reopen")
  })

  it("ignores anything else, visibility included, which is handled elsewhere", () => {
    expect(lifecycleAction("visibilitychange")).toBe("ignore")
    expect(lifecycleAction("focus")).toBe("ignore")
    expect(lifecycleAction("online")).toBe("ignore")
    expect(lifecycleAction("unload")).toBe("ignore")
  })

  it("listens to exactly the events it has an action for", () => {
    for (const event of LIFECYCLE_EVENTS) {
      expect(lifecycleAction(event)).not.toBe("ignore")
    }
  })

  it("reopens on pageshow whether or not the page was persisted", () => {
    // `persisted` is deliberately not part of the decision: a bfcache restore
    // and an ordinary back navigation both arrive with nothing open.
    const restored = spy()
    applyLifecycle(restored, "pageshow")
    expect(restored.calls).toEqual(["resumeNow"])
  })
})

describe("the wiring", () => {
  it("drives every registered socket from the real window events", () => {
    const a = spy()
    const b = spy()
    const offA = registerPageLifecycle(a)
    const offB = registerPageLifecycle(b)
    window.dispatchEvent(new Event("pagehide"))
    window.dispatchEvent(new Event("pageshow"))
    window.dispatchEvent(new Event("freeze"))
    window.dispatchEvent(new Event("resume"))
    expect(a.calls).toEqual(["close", "resumeNow", "park", "resumeNow"])
    expect(b.calls).toEqual(a.calls)
    offA()
    offB()
  })

  it("stops driving a socket that unregistered, which every pane's does on unmount", () => {
    const pane = spy()
    const off = registerPageLifecycle(pane)
    off()
    window.dispatchEvent(new Event("pagehide"))
    expect(pane.calls).toEqual([])
  })
})
