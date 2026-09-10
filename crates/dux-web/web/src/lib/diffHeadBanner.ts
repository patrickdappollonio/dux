import type { FileDiffHead } from "./fileApi"

// The one sentence shown above a diff that was cut short, word for word the one
// the terminal UI puts there (`dux_core::diff::diff_head_banner`). Empty when
// nothing was cut, so a caller can render it unconditionally.
export function diffHeadBanner(head: FileDiffHead): string {
  if (!head.truncated) return ""
  const lines = head.total_lines === 1 ? "1 line" : `${head.total_lines} lines`
  const total = head.total_is_at_least ? `more than ${lines}` : lines
  return (
    `Diff cut here: showing the first ${head.shown_lines} of ${total}. ` +
    "Open the file in your editor or run git diff to see the rest."
  )
}
