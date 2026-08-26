// THE FOUR CONNECTION TIMINGS, and the one place their defaults live.
//
// Each mirrors a `[server]` key and each has a documented default that is used
// whenever the server does not answer: an older build that predates the key, or
// any render before the first bootstrap fetch lands. The defaults are plain
// duplicated literals of the Rust ones (`dux_core::config::ServerConfig`), not
// generated from them, so nothing enforces the two staying equal; the test
// beside this file pins the browser's half.
//
// Published at module scope from the store when the bootstrap document lands,
// on the same idiom (and for the same reason) as `notify.ts`'s status window: a
// value read out of a render closure by a long-lived socket or timer callback is
// pinned to whatever it was at mount, forever. With the value living where it is
// read there is nothing to capture, so there is nothing to capture stale.
import type { Bootstrap } from "./bootstrapApi"

/// Seconds of VISIBLE time a pane waits for its screen after connecting before
/// it swaps the spinner for a Reconnect box. `0` disables the wait, leaving the
/// cover up indefinitely.
export const DEFAULT_REPLAY_WAIT_SECONDS = 8

/// The longest gap between two automatic reconnect attempts. The backoff doubles
/// up to this and then stays there, indefinitely, while the page is visible.
export const DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS = 10

/// How often a visible page sends its one periodic frame while it is NOT the
/// owner-and-visible pair that owes the engine a faster viewed ping.
export const DEFAULT_HEARTBEAT_SECONDS = 15

/// Seconds of VISIBLE time to wait for the server's answer to a beat before
/// treating the socket as half-open and forcing a plain reconnect.
export const DEFAULT_HEARTBEAT_DEADLINE_SECONDS = 30

/// The part of the bootstrap document this module reads. A partial rather than
/// the whole document, so a test can publish four numbers without building one.
export type ConnectionTimingDoc = Partial<
  Pick<
    Bootstrap,
    | "replay_wait_seconds"
    | "reconnect_backoff_cap_seconds"
    | "heartbeat_seconds"
    | "heartbeat_deadline_seconds"
  >
>

let published: ConnectionTimingDoc | undefined = undefined

/// Publish (or, with `undefined`, retract) the server's answers. Called when the
/// bootstrap document lands and again on every refetch after a config change.
export function publishConnectionTiming(doc: ConnectionTimingDoc | undefined): void {
  published = doc
}

/// A configured value in milliseconds, or the default. `allowZero` says whether
/// zero is a real answer: it is for the replay wait, where it means "wait
/// forever", and it is not for a period or a deadline, where it would mean a hot
/// loop. Anything negative or non-finite is not an answer at all.
function seconds(value: number | undefined, fallback: number, allowZero: boolean): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback * 1000
  if (value < 0) return fallback * 1000
  if (value === 0 && !allowZero) return fallback * 1000
  return value * 1000
}

/// `[server] replay_wait_seconds` in ms. Zero means the wait is disabled.
export function replayWaitMs(): number {
  return seconds(published?.replay_wait_seconds, DEFAULT_REPLAY_WAIT_SECONDS, true)
}

/// `[server] reconnect_backoff_cap_seconds` in ms.
export function reconnectBackoffCapMs(): number {
  return seconds(
    published?.reconnect_backoff_cap_seconds,
    DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS,
    false,
  )
}

/// `[server] heartbeat_seconds` in ms.
export function heartbeatPeriodMs(): number {
  return seconds(published?.heartbeat_seconds, DEFAULT_HEARTBEAT_SECONDS, false)
}

/// What an inverted pair is clamped to, as a multiple of the beat period.
const INVERTED_DEADLINE_PERIODS = 2

/// `[server] heartbeat_deadline_seconds` in ms.
///
/// A deadline AT OR BELOW the period is a permanent reconnect loop rather than a
/// tight setting: the deadline is checked on the send timer, so the very first
/// tick after a frame goes out already finds it elapsed and drops a perfectly
/// healthy socket, forever. Zero and negatives were already refused here; this
/// pair was not, and the docs already promise the deadline is comfortably larger
/// than the interval, so a pair that says otherwise is a typo. It is CLAMPED
/// rather than rejected: a working terminal on a corrected timing beats no
/// terminal on a refused config. A deadline merely a little larger than the
/// period is left exactly as written; it costs an extra period before a miss is
/// noticed and nothing else.
export function heartbeatDeadlineMs(): number {
  const configured = seconds(
    published?.heartbeat_deadline_seconds,
    DEFAULT_HEARTBEAT_DEADLINE_SECONDS,
    false,
  )
  const period = heartbeatPeriodMs()
  if (configured > period) return configured
  return period * INVERTED_DEADLINE_PERIODS
}
