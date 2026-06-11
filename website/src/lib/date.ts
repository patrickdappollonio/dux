// Shared date formatting for the blog (index list, post header, RSS dates).
// Keeping one helper means the listing, the post page, and the feed never drift
// in how a date reads.

// Format a publish/updated date as a long, human date, e.g. "June 8, 2026".
//
// Frontmatter dates written as a bare `2026-06-08` are parsed as UTC midnight.
// Formatting in UTC keeps the displayed day stable regardless of the build
// machine's timezone — otherwise a build west of UTC would render the previous
// day.
export function formatDate(date: Date): string {
  return date.toLocaleDateString("en-US", {
    year: "numeric",
    month: "long",
    day: "numeric",
    timeZone: "UTC",
  });
}

// Machine-readable `YYYY-MM-DD` for the <time datetime> attribute, also in UTC
// for the same stability reason.
export function isoDate(date: Date): string {
  return date.toISOString().slice(0, 10);
}
