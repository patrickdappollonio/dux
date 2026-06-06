import { describe, expect, it } from "vitest"

import {
  LOGIN_INVALID_MESSAGE,
  loginErrorMessage,
  parseRetryAfter,
  phaseFromMe,
} from "./auth"

describe("phaseFromMe", () => {
  it("maps auth-disabled to the disabled phase", () => {
    expect(phaseFromMe(200, { auth: "disabled" })).toEqual({
      phase: "disabled",
      username: null,
    })
  })

  it("maps a 200 with a username to authed", () => {
    expect(phaseFromMe(200, { username: "alice" })).toEqual({
      phase: "authed",
      username: "alice",
    })
  })

  it("maps a 401 to anonymous", () => {
    expect(phaseFromMe(401, null)).toEqual({
      phase: "anonymous",
      username: null,
    })
  })

  it("treats an unexpected 200 body as anonymous (fail safe)", () => {
    // Neither `auth: disabled` nor a username — fall back to the login screen
    // rather than a half-authed app.
    expect(phaseFromMe(200, {})).toEqual({ phase: "anonymous", username: null })
    expect(phaseFromMe(200, { username: "" })).toEqual({
      phase: "anonymous",
      username: null,
    })
  })

  it("treats a non-200/401 status as anonymous", () => {
    expect(phaseFromMe(500, null)).toEqual({
      phase: "anonymous",
      username: null,
    })
  })
})

describe("parseRetryAfter", () => {
  it("parses a delta-seconds integer", () => {
    expect(parseRetryAfter("30")).toBe(30)
    expect(parseRetryAfter("  45 ")).toBe(45)
  })

  it("falls back when the header is missing", () => {
    expect(parseRetryAfter(null)).toBe(60)
    expect(parseRetryAfter(null, 90)).toBe(90)
  })

  it("falls back on an unparseable or non-positive value", () => {
    expect(parseRetryAfter("not-a-number")).toBe(60)
    expect(parseRetryAfter("0")).toBe(60)
    expect(parseRetryAfter("-5")).toBe(60)
  })
})

describe("loginErrorMessage", () => {
  it("returns the generic invalid-credentials message for 401", () => {
    expect(loginErrorMessage(401)).toBe(LOGIN_INVALID_MESSAGE)
  })

  it("returns a throttle message naming the retry window for 429", () => {
    expect(loginErrorMessage(429, 42)).toBe(
      "Too many attempts — try again in 42 s.",
    )
  })

  it("defaults the retry window when none is given for 429", () => {
    expect(loginErrorMessage(429)).toBe("Too many attempts — try again in 60 s.")
  })

  it("returns a generic try-again message for other statuses", () => {
    expect(loginErrorMessage(500)).toBe("Could not sign in. Please try again.")
  })
})
