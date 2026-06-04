import { describe, expect, it } from "vitest"

import { isValidAgentName, sanitizeAgentName } from "./agentName"

describe("sanitizeAgentName", () => {
  const cases: Array<[string, string]> = [
    // Plain names pass through unchanged.
    ["feature-x", "feature-x"],
    ["AbC123", "AbC123"],
    ["a_b-c", "a_b-c"],
    ["feat/sub", "feat/sub"],
    // Spaces become dashes (transparent, like the TUI char map).
    ["my feature", "my-feature"],
    ["a b c", "a-b-c"],
    // Disallowed characters are dropped.
    ["with.dot", "withdot"],
    ["a!b@c#", "abc"],
    // Weird unicode is dropped (only ASCII alnum/-/_// survive).
    ["naïve", "nave"],
    ["emoji😀x", "emojix"],
    ["héllo", "hllo"],
    // Leading non-alphanumerics are dropped until the first alphanumeric.
    ["-leading", "leading"],
    ["__x", "x"],
    ["/leading", "leading"],
    ["---", ""],
    // Consecutive slashes collapse to a single slash.
    ["a//b", "a/b"],
    ["a///b", "a/b"],
    ["feat//sub//thing", "feat/sub/thing"],
    // A single trailing slash is PRESERVED (mid-typing); isValidAgentName is
    // what rejects it for submit.
    ["feat/", "feat/"],
    // Empty stays empty.
    ["", ""],
  ]

  for (const [input, expected] of cases) {
    it(`maps ${JSON.stringify(input)} -> ${JSON.stringify(expected)}`, () => {
      expect(sanitizeAgentName(input)).toBe(expected)
    })
  }

  it("is idempotent (sanitizing a sanitized value is a no-op)", () => {
    for (const [input] of cases) {
      const once = sanitizeAgentName(input)
      expect(sanitizeAgentName(once)).toBe(once)
    }
  })
})

describe("isValidAgentName", () => {
  // Exact port of Rust's `is_valid_agent_name`, which forbids a LEADING `-` or
  // `/` but NOT a leading `_` (underscore is whitelisted everywhere). The TUI's
  // per-keystroke char map additionally rejects a leading `_`, and our
  // `sanitizeAgentName` drops it too — so the input never produces a leading `_`
  // — but `isValidAgentName` mirrors Rust and accepts one if handed it directly.
  const valid = [
    "feature-x",
    "AbC123",
    "a_b-c",
    "feat/sub-thing",
    "x",
    "_underscoreLead", // valid per is_valid_agent_name (sanitize would drop it)
  ]
  const invalid = [
    "", // empty
    "-leading", // leading dash
    "/leading", // leading slash
    "trailing/", // trailing slash (rejected for submit, not by sanitize)
    "a//b", // consecutive slashes
    "has space", // raw space
    "with.dot", // disallowed char
    "naïve", // non-ascii
  ]

  for (const name of valid) {
    it(`accepts ${JSON.stringify(name)}`, () => {
      expect(isValidAgentName(name)).toBe(true)
    })
  }
  for (const name of invalid) {
    it(`rejects ${JSON.stringify(name)}`, () => {
      expect(isValidAgentName(name)).toBe(false)
    })
  }

  it("accepts every non-empty sanitized value that is not solely a trailing-slash case", () => {
    // A sanitized value is submit-valid unless it ends with a single trailing
    // slash (which sanitize keeps but validation rejects).
    for (const input of ["my feature", "a//b", "-leading", "feat/sub"]) {
      const s = sanitizeAgentName(input)
      if (s.length > 0 && !s.endsWith("/")) {
        expect(isValidAgentName(s)).toBe(true)
      }
    }
  })
})
