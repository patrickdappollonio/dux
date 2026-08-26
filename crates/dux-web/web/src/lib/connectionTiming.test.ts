import { afterEach, describe, expect, it } from "vitest"

import {
  DEFAULT_HEARTBEAT_DEADLINE_SECONDS,
  DEFAULT_HEARTBEAT_SECONDS,
  DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS,
  DEFAULT_REPLAY_WAIT_SECONDS,
  heartbeatDeadlineMs,
  heartbeatPeriodMs,
  publishConnectionTiming,
  reconnectBackoffCapMs,
  replayWaitMs,
} from "./connectionTiming"

afterEach(() => {
  publishConnectionTiming(undefined)
})

describe("the documented defaults", () => {
  it("are the four values an older server or a pre-bootstrap render falls back to", () => {
    expect(DEFAULT_REPLAY_WAIT_SECONDS).toBe(8)
    expect(DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS).toBe(10)
    expect(DEFAULT_HEARTBEAT_SECONDS).toBe(15)
    expect(DEFAULT_HEARTBEAT_DEADLINE_SECONDS).toBe(30)
  })

  it("are what every reader returns before the bootstrap document lands", () => {
    expect(replayWaitMs()).toBe(8_000)
    expect(reconnectBackoffCapMs()).toBe(10_000)
    expect(heartbeatPeriodMs()).toBe(15_000)
    expect(heartbeatDeadlineMs()).toBe(30_000)
  })

  it("are what a server that omits the keys falls back to", () => {
    publishConnectionTiming({})
    expect(replayWaitMs()).toBe(8_000)
    expect(reconnectBackoffCapMs()).toBe(10_000)
    expect(heartbeatPeriodMs()).toBe(15_000)
    expect(heartbeatDeadlineMs()).toBe(30_000)
  })
})

describe("a published document", () => {
  it("is read by every getter", () => {
    publishConnectionTiming({
      replay_wait_seconds: 3,
      reconnect_backoff_cap_seconds: 4,
      heartbeat_seconds: 5,
      heartbeat_deadline_seconds: 6,
    })
    expect(replayWaitMs()).toBe(3_000)
    expect(reconnectBackoffCapMs()).toBe(4_000)
    expect(heartbeatPeriodMs()).toBe(5_000)
    expect(heartbeatDeadlineMs()).toBe(6_000)
  })

  it("keeps a configured zero for the replay wait, which DISABLES it", () => {
    publishConnectionTiming({ replay_wait_seconds: 0 })
    expect(replayWaitMs()).toBe(0)
  })

  it("refuses a negative or non-finite value and falls back", () => {
    publishConnectionTiming({
      replay_wait_seconds: -1,
      heartbeat_seconds: Number.NaN,
    })
    expect(replayWaitMs()).toBe(8_000)
    expect(heartbeatPeriodMs()).toBe(15_000)
  })

  it("floors the three periods that cannot meaningfully be zero", () => {
    // A zero backoff cap would be a hot retry loop, a zero heartbeat period a
    // frame per tick, and a zero deadline a reconnect on every beat.
    publishConnectionTiming({
      reconnect_backoff_cap_seconds: 0,
      heartbeat_seconds: 0,
      heartbeat_deadline_seconds: 0,
    })
    expect(reconnectBackoffCapMs()).toBe(10_000)
    expect(heartbeatPeriodMs()).toBe(15_000)
    expect(heartbeatDeadlineMs()).toBe(30_000)
  })
})

// AN INVERTED PAIR IS A PERMANENT RECONNECT LOOP. The frame goes out, the next
// tick arrives no earlier than the deadline, and the socket is dropped for a
// miss that never had time to arrive. Zero and negative values were already
// refused; this pair was not, and the docs already promise the deadline is
// comfortably larger than the interval.
describe("the heartbeat deadline against its own period", () => {
  it("is clamped up when a config makes it smaller than the period", () => {
    publishConnectionTiming({ heartbeat_seconds: 30, heartbeat_deadline_seconds: 5 })
    expect(heartbeatDeadlineMs()).toBe(60_000)
  })

  it("is clamped up when the two are EQUAL, which loops just as surely", () => {
    publishConnectionTiming({ heartbeat_seconds: 20, heartbeat_deadline_seconds: 20 })
    expect(heartbeatDeadlineMs()).toBe(40_000)
  })

  it("leaves a sane pair exactly as configured", () => {
    publishConnectionTiming({ heartbeat_seconds: 10, heartbeat_deadline_seconds: 45 })
    expect(heartbeatDeadlineMs()).toBe(45_000)
  })

  it("leaves a merely TIGHT pair alone, because that one still works", () => {
    publishConnectionTiming({ heartbeat_seconds: 5, heartbeat_deadline_seconds: 6 })
    expect(heartbeatDeadlineMs()).toBe(6_000)
  })
})
