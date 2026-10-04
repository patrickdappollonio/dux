import { afterEach, describe, expect, it } from "vitest"

import {
  changesRequestTimeoutMs,
  DEFAULT_HEARTBEAT_DEADLINE_SECONDS,
  DEFAULT_HEARTBEAT_SECONDS,
  DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS,
  DEFAULT_RECONNECT_ATTEMPTS,
  DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS,
  DEFAULT_REPLAY_WAIT_SECONDS,
  heartbeatDeadlineMs,
  heartbeatPeriodMs,
  publishConnectionTiming,
  reconnectAttemptBudget,
  reconnectAttemptTimeoutMs,
  reconnectBackoffCapMs,
  replayWaitMs,
} from "./connectionTiming"

afterEach(() => {
  publishConnectionTiming(undefined)
})

describe("the documented defaults", () => {
  it("are the values an older server or a pre-bootstrap render falls back to", () => {
    expect(DEFAULT_REPLAY_WAIT_SECONDS).toBe(8)
    expect(DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS).toBe(10)
    expect(DEFAULT_HEARTBEAT_SECONDS).toBe(15)
    expect(DEFAULT_HEARTBEAT_DEADLINE_SECONDS).toBe(30)
    expect(DEFAULT_RECONNECT_ATTEMPTS).toBe(8)
    expect(DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS).toBe(10)
  })

  it("are what every reader returns before the bootstrap document lands", () => {
    expect(replayWaitMs()).toBe(8_000)
    expect(reconnectBackoffCapMs()).toBe(10_000)
    expect(heartbeatPeriodMs()).toBe(15_000)
    expect(heartbeatDeadlineMs()).toBe(30_000)
    expect(reconnectAttemptBudget()).toBe(8)
    expect(reconnectAttemptTimeoutMs()).toBe(10_000)
  })

  it("are what a server that omits the keys falls back to", () => {
    publishConnectionTiming({})
    expect(replayWaitMs()).toBe(8_000)
    expect(reconnectBackoffCapMs()).toBe(10_000)
    expect(heartbeatPeriodMs()).toBe(15_000)
    expect(heartbeatDeadlineMs()).toBe(30_000)
    expect(reconnectAttemptBudget()).toBe(8)
    expect(reconnectAttemptTimeoutMs()).toBe(10_000)
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

  it("reads the budget and the attempt deadline too", () => {
    publishConnectionTiming({
      reconnect_attempts: 3,
      reconnect_attempt_timeout_seconds: 4,
    })
    expect(reconnectAttemptBudget()).toBe(3)
    expect(reconnectAttemptTimeoutMs()).toBe(4_000)
  })

  it("keeps a configured zero for the budget, which means never give up", () => {
    publishConnectionTiming({ reconnect_attempts: 0 })
    expect(reconnectAttemptBudget()).toBe(0)
  })

  it("refuses a negative, fractional or non-finite budget and falls back", () => {
    publishConnectionTiming({ reconnect_attempts: -2 })
    expect(reconnectAttemptBudget()).toBe(8)
    publishConnectionTiming({ reconnect_attempts: 2.5 })
    expect(reconnectAttemptBudget()).toBe(8)
    publishConnectionTiming({ reconnect_attempts: Number.NaN })
    expect(reconnectAttemptBudget()).toBe(8)
  })

  it("reads the changes request deadline, defaulting to thirty seconds", () => {
    publishConnectionTiming(undefined)
    expect(changesRequestTimeoutMs()).toBe(30_000)
    publishConnectionTiming({ changes_request_timeout_seconds: 12 })
    expect(changesRequestTimeoutMs()).toBe(12_000)
  })

  it("falls back on a changes request deadline that is no answer at all", () => {
    publishConnectionTiming({ changes_request_timeout_seconds: -4 })
    expect(changesRequestTimeoutMs()).toBe(30_000)
    publishConnectionTiming({ changes_request_timeout_seconds: Number.NaN })
    expect(changesRequestTimeoutMs()).toBe(30_000)
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

  // Zero meaning the default, the changes ceiling and the inverted
  // heartbeat pair are the SERVER's rules now (dux_core::config_effective,
  // tested there): the browser runs what it is sent, so a value the server
  // sends is never second-guessed here.
  it("runs every published value as sent, the heartbeat pair included", () => {
    publishConnectionTiming({
      heartbeat_seconds: 30,
      heartbeat_deadline_seconds: 60,
      changes_request_timeout_seconds: 600,
    })
    expect(heartbeatPeriodMs()).toBe(30_000)
    expect(heartbeatDeadlineMs()).toBe(60_000)
    expect(changesRequestTimeoutMs()).toBe(600_000)
  })
})
