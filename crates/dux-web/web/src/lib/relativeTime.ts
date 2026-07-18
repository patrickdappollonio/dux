// A compact "time ago" formatter for the flat agent list's per-row timestamp
// (from `SessionView.updated_at` / a terminal's activity). It renders the short,
// inbox-style form the mockup uses: "now", "5m", "3h", "4d", "2w". Kept pure and
// React-free so it is trivially unit-testable and shared by both surfaces.
//
// The scale is deliberately coarse (no seconds past "now", weeks as the largest
// unit): the row only needs a glanceable recency cue, not a precise duration.
// An unparseable or future timestamp collapses to "now" rather than emitting a
// negative or NaN value.
export function relativeTime(iso: string, now: number = Date.now()): string {
  const then = Date.parse(iso)
  if (Number.isNaN(then)) return ""
  const deltaMs = now - then
  // Future or sub-minute: a single "now" avoids a jittery seconds counter.
  if (deltaMs < 45_000) return "now"

  const minutes = Math.floor(deltaMs / 60_000)
  if (minutes < 60) return `${minutes}m`

  const hours = Math.floor(deltaMs / 3_600_000)
  if (hours < 24) return `${hours}h`

  const days = Math.floor(deltaMs / 86_400_000)
  if (days < 7) return `${days}d`

  const weeks = Math.floor(days / 7)
  return `${weeks}w`
}
