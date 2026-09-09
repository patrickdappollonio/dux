// The connection timings and the one place their defaults live. Each mirrors a
// `[server]` key, and its default applies whenever the server does not answer. The
// defaults are duplicated literals of `dux_core::config::ServerConfig`, so nothing
// enforces the two staying equal.
//
// Published at module scope so a long-lived socket or timer callback reads the live
// value rather than pinning whatever its render closure captured at mount.
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

/// `[server] heartbeat_deadline_seconds` in ms, clamped above the send period: the
/// deadline is checked on the send timer, so a deadline at or below the period finds
/// itself elapsed on the first tick and drops a healthy socket forever. Clamped
/// rather than refused, since a working terminal beats a rejected config.
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
