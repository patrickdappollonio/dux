import { describe, expect, it } from "vitest"

import { statusToastAllowed } from "./statusRouting"

describe("statusToastAllowed on the workspace surface", () => {
  it("renders every status, whatever its scope", () => {
    expect(statusToastAllowed("all", false)).toBe(true)
    expect(statusToastAllowed({ connection: "c1" }, false)).toBe(true)
    expect(statusToastAllowed(undefined, false)).toBe(true)
    expect(statusToastAllowed(null, false)).toBe(true)
    expect(statusToastAllowed("something-new", false)).toBe(true)
  })
})

describe("statusToastAllowed in the standalone editor tab", () => {
  it("drops the broadcast scope", () => {
    expect(statusToastAllowed("all", true)).toBe(false)
  })

  it("drops a status whose scope is missing, which is a malformed frame", () => {
    expect(statusToastAllowed(undefined, true)).toBe(false)
    expect(statusToastAllowed(null, true)).toBe(false)
  })

  it("renders a status addressed to this connection", () => {
    expect(statusToastAllowed({ connection: "c1" }, true)).toBe(true)
  })

  it("renders any other scope shape a later server might add", () => {
    // The rule is "anything except the literal broadcast string is addressed",
    // so a new addressed form does not need a client change to reach the user.
    expect(statusToastAllowed({ session: "s1" }, true)).toBe(true)
    expect(statusToastAllowed("connection", true)).toBe(true)
  })
})
