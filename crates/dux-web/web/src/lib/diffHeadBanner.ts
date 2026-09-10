import type { FileDiffHead } from "./fileApi"

// The one sentence shown ABOVE a diff that was cut short, word for word the one
// the terminal UI puts there (`dux_core::diff::diff_head_banner`). Empty when
// nothing was cut, so a caller can render it unconditionally.
//
// Above rather than below: a reader who has to scroll four thousand rows to
// learn the diff was cut has already been misled by everything before it. The
// counts are grouped with commas, the same way the Rust half groups them, so
// the two strings stay identical.
export function diffHeadBanner(head: FileDiffHead): string {
  if (!head.truncated) return ""
  const grouped = head.total_lines.toLocaleString("en-US")
  const lines = head.total_lines === 1 ? `${grouped} line` : `${grouped} lines`
  const total = head.total_is_at_least ? `more than ${lines}` : lines
  return (
    `Diff cut here: showing the first ${head.shown_lines.toLocaleString("en-US")} of ${total}. ` +
    "Open the file in your editor or run git diff to see the rest."
  )
}
