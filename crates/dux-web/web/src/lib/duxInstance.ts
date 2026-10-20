// Which dux this page opened on, and whether the dux answering now is that one.
//
// Every dux keeps a random instance id across its restarts and reports it on
// the status and build reads. The page remembers the first one it is told and
// compares every later report against it, so a restart of its own dux (same
// id) is told apart from a different dux answering at the same address (another
// id). The comparison is of what the dux REPORTS, never of the page's own
// address: the address is exactly what stayed the same when the dux behind it
// changed.
//
// A consistency check only. The id is public and anyone can repeat it, so it
// never decides anything about trust or sign-in; it only decides which cover
// the page shows.

export type InstanceVerdict = "same" | "different" | "unknown"

/// The verdict for a report against the remembered id. Unknown on either side
/// is never "different": a dux too old to report one, or a read that did not
/// answer, is no evidence of a change.
export function compareInstances(
  opened: string | null,
  reported: string | null,
): InstanceVerdict {
  if (opened === null || reported === null) return "unknown"
  return opened === reported ? "same" : "different"
}

let opened: string | null = null

/// Weigh a report: the first id the page is told becomes the one it opened on,
/// and every report after that is compared against it.
export function noteReportedInstance(reported: string | null): InstanceVerdict {
  if (opened === null) opened = reported
  return compareInstances(opened, reported)
}

/// The id this page opened on, or null while it has not been told one.
export function openedInstance(): string | null {
  return opened
}

/// For tests: forget the dux this page opened on.
export function resetOpenedInstanceForTests(): void {
  opened = null
}
