import { describe, expect, it } from "vitest"

import { launcherVerb } from "@/lib/launcherVerb"

describe("launcherVerb", () => {
  // The whole point of the helper: an unloaded spine is NOT a zero. A populated
  // workspace would otherwise render "Add project" for one frame on every load.
  it("reads as New agent while the project count is unknown", () => {
    expect(launcherVerb(null)).toBe("new-agent")
  })

  it("flips to Add project only on a confirmed zero", () => {
    expect(launcherVerb(0)).toBe("add-project")
  })

  it("stays New agent as soon as any project exists", () => {
    expect(launcherVerb(1)).toBe("new-agent")
    expect(launcherVerb(7)).toBe("new-agent")
  })
})
