import { describe, expect, it } from "vitest"

import {
  RECONNECT_MIN_MS,
  budgetSpent,
  planNextAttempt,
  retryDelayMs,
} from "./reconnectSchedule"

describe("the backoff shape", () => {
  it("starts at the floor and doubles", () => {
    expect(RECONNECT_MIN_MS).toBe(500)
    expect(retryDelayMs(1, 10_000)).toBe(500)
    expect(retryDelayMs(2, 10_000)).toBe(1_000)
    expect(retryDelayMs(3, 10_000)).toBe(2_000)
    expect(retryDelayMs(4, 10_000)).toBe(4_000)
  })

  it("stops doubling at the configured cap and stays there", () => {
    expect(retryDelayMs(5, 4_000)).toBe(4_000)
    expect(retryDelayMs(50, 4_000)).toBe(4_000)
  })

  it("never returns less than the floor, whatever the cap says", () => {
    // A cap below the floor is a config nobody should write, but a hot loop is
    // the one answer it must not produce.
    expect(retryDelayMs(1, 10)).toBe(RECONNECT_MIN_MS)
  })
})

describe("the budget", () => {
  it("is spent once as many attempts have failed as it allows", () => {
    expect(budgetSpent(7, 8)).toBe(false)
    expect(budgetSpent(8, 8)).toBe(true)
    expect(budgetSpent(9, 8)).toBe(true)
  })

  it("is never spent when it is zero, which means never give up", () => {
    expect(budgetSpent(1, 0)).toBe(false)
    expect(budgetSpent(1_000, 0)).toBe(false)
  })

  it("is not spent before anything has failed", () => {
    expect(budgetSpent(0, 8)).toBe(false)
  })
})

describe("planNextAttempt", () => {
  it("schedules the next attempt while the budget holds", () => {
    expect(planNextAttempt({ failures: 1, capMs: 10_000, budget: 8 })).toEqual({
      kind: "retry",
      attempt: 2,
      delayMs: 500,
    })
    expect(planNextAttempt({ failures: 3, capMs: 10_000, budget: 8 })).toEqual({
      kind: "retry",
      attempt: 4,
      delayMs: 2_000,
    })
  })

  it("gives up on the failure that spends the budget, naming the attempts made", () => {
    expect(planNextAttempt({ failures: 8, capMs: 10_000, budget: 8 })).toEqual({
      kind: "give_up",
      attempts: 8,
    })
  })

  it("never gives up on an unlimited budget", () => {
    expect(planNextAttempt({ failures: 99, capMs: 10_000, budget: 0 })).toEqual({
      kind: "retry",
      attempt: 100,
      delayMs: 10_000,
    })
  })

  // What a reset looks like from here: the caller zeroes its failure count, so
  // a schedule that had grown out to the cap answers from the floor again.
  it("is back at the floor and the second attempt once the count is reset", () => {
    expect(planNextAttempt({ failures: 6, capMs: 10_000, budget: 0 })).toEqual({
      kind: "retry",
      attempt: 7,
      delayMs: 10_000,
    })
    expect(planNextAttempt({ failures: 1, capMs: 10_000, budget: 0 })).toEqual({
      kind: "retry",
      attempt: 2,
      delayMs: 500,
    })
  })
})
