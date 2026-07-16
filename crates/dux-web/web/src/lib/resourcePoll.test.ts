import { describe, expect, it } from "vitest"

import {
  RESOURCE_POLL_INTERVAL_MS,
  STALE_STATS_THRESHOLD_MS,
  nextPollDelay,
  pollIntervalLabel,
  shouldPoll,
  statsAreStale,
} from "./resourcePoll"

describe("shouldPoll", () => {
  it("polls_only_while_open", () => {
    expect(shouldPoll({ open: true, hidden: false })).toBe(true)
    // Closed: the whole point of the REST design is that a closed dialog costs
    // nothing.
    expect(shouldPoll({ open: false, hidden: false })).toBe(false)
  })

  it("pauses_while_document_hidden", () => {
    // A backgrounded tab keeps the dialog mounted; polling it would walk the
    // server's process table every second for nobody.
    expect(shouldPoll({ open: true, hidden: true })).toBe(false)
  })

  it("resumes_on_visibility_return", () => {
    const hidden = { open: true, hidden: true }
    expect(shouldPoll(hidden)).toBe(false)
    expect(shouldPoll({ ...hidden, hidden: false })).toBe(true)
  })

  it("stays_paused_when_closed_even_if_visible", () => {
    expect(shouldPoll({ open: false, hidden: true })).toBe(false)
  })
})

describe("nextPollDelay", () => {
  it("uses_wall_clock_time_since_the_last_sample", () => {
    // Wall-clock, not tick counts: a slow fetch must not push the cadence out.
    // A sample that returned instantly waits the full interval.
    expect(nextPollDelay(RESOURCE_POLL_INTERVAL_MS, 0)).toBe(
      RESOURCE_POLL_INTERVAL_MS,
    )
    // One that took 400ms waits only the remainder, so the cadence holds.
    expect(nextPollDelay(RESOURCE_POLL_INTERVAL_MS, 400)).toBe(
      RESOURCE_POLL_INTERVAL_MS - 400,
    )
  })

  it("never_returns_a_negative_delay", () => {
    // A fetch slower than the interval polls again immediately, never
    // "in the past".
    expect(nextPollDelay(1000, 5000)).toBe(0)
  })

  it("pins_the_cadence_at_one_second", () => {
    // The server caches for 1s, so polling faster only burns requests.
    expect(RESOURCE_POLL_INTERVAL_MS).toBe(1000)
  })
})

describe("statsAreStale", () => {
  it("is_never_stale_before_the_first_sample_ever_lands", () => {
    // Nothing to judge staleness against yet; the dialog is still showing its
    // initial dashes, not stale numbers.
    expect(statsAreStale(1_000_000, null)).toBe(false)
  })

  it("is_not_stale_immediately_after_a_success", () => {
    expect(statsAreStale(1_000_000, 1_000_000)).toBe(false)
  })

  it("is_not_stale_after_a_single_missed_interval", () => {
    // One dropped poll is normal jitter, not a stall worth alarming over.
    const lastSuccessAt = 1_000_000
    expect(
      statsAreStale(lastSuccessAt + RESOURCE_POLL_INTERVAL_MS, lastSuccessAt),
    ).toBe(false)
  })

  it("becomes_stale_after_repeated_missed_intervals", () => {
    const lastSuccessAt = 1_000_000
    expect(
      statsAreStale(lastSuccessAt + STALE_STATS_THRESHOLD_MS + 1, lastSuccessAt),
    ).toBe(true)
  })
})

describe("pollIntervalLabel", () => {
  it("derives_the_label_from_the_real_poll_constant", () => {
    // Must be computed FROM the constant, not a hand-typed number: this is the
    // whole point of the helper (the header pill cannot drift from the actual
    // cadence).
    expect(pollIntervalLabel(RESOURCE_POLL_INTERVAL_MS)).toBe("every 1s")
  })

  it("formats_a_sub_second_interval_with_one_decimal", () => {
    expect(pollIntervalLabel(1500)).toBe("every 1.5s")
  })
})
