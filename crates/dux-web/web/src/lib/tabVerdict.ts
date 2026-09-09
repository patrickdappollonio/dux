import type { TabRunVerdict } from "@/lib/types"

// The dormant card's words, ported one-for-one from `dux_core::tab_verdict` and
// pinned by the tests below, so a user reading the fact in the terminal
// recognises it in the browser. If either side changes, both change.
//
// Deliberately not the app's compact `relativeTime` ("2m"): that is a column in
// a list, and this is a clause inside a sentence. The apostrophes are
// typographic where the Rust uses ASCII, and the test normalises before
// comparing, so punctuation stays house style without the wording drifting.

const MINUTE = 60
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/** "moments ago", "about 2 minutes ago". Coarse on purpose: the card answers
 * "was this just now, or this morning", not a stopwatch reading. Every boundary
 * is the ROUNDED value's, or the rounding walks a value into the next unit's
 * vocabulary and prints "about 60 minutes ago". */
export function humanizeAgeAgo(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  if (s < 45) return "moments ago"
  if (s < 90) return "about a minute ago"
  if (s < HOUR - MINUTE / 2) {
    return `about ${Math.floor((s + MINUTE / 2) / MINUTE)} minutes ago`
  }
  if (s < 90 * MINUTE) return "about an hour ago"
  if (s < DAY - HOUR / 2) {
    return `about ${Math.floor((s + HOUR / 2) / HOUR)} hours ago`
  }
  if (s < 36 * HOUR) return "about a day ago"
  return `about ${Math.floor((s + DAY / 2) / DAY)} days ago`
}

/** The one sentence the card prints about how the last run ended. Neutral about
 * blame, since a non-zero exit is often the user quitting a CLI: it says what
 * dux observed and what dux therefore did not do, never that anything crashed.
 * An `ending` this build does not know (a newer server) takes the generic
 * sentence rather than printing a wire kind at a person. */
export function endingSentence(verdict: TabRunVerdict): string {
  const age = humanizeAgeAgo(verdict.ended_seconds_ago)
  switch (verdict.ending) {
    case "launch_failed":
      return `Its last run could not be launched ${age}: ${verdict.error ?? "no reason given"}`
    case "exited":
      // Fall back rather than fill in a missing status: "status 0" would be the
      // most misleading number there is, since zero means the run succeeded.
      return verdict.status == null
        ? genericEndingSentence()
        : `Its last run exited with status ${verdict.status} ${age}, so dux didn\u2019t start it again on its own.`
    case "exited_unknown":
      return `Its last run exited with an unknown status ${age}, so dux didn\u2019t start it again on its own.`
    case "rapid_clean_exit":
      // The window's TRUTH is `RAPID_EXIT_WINDOW` in
      // crates/dux-core/src/engine/lifecycle.rs; the literal here is pinned to
      // it by a test rather than derived, because the wire does not carry it.
      return `Its last run ended ${age} in under five seconds with status 0, which dux treats as a run that never came up.`
    default:
      return genericEndingSentence()
  }
}

/** The fallback wording, for a server that sends `last_run_failed: true` with no
 * verdict beside it and for a verdict this build cannot word (an unknown kind,
 * an `exited` with no status). It stays honest in each case: something ended
 * badly and dux did not start it again. */
export function genericEndingSentence(): string {
  return "Its last run ended with an error or a non-zero exit, so dux didn\u2019t start it again on its own."
}
