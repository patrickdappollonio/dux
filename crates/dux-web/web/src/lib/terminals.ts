import type { TerminalView } from "@/lib/types"

// The terminal's display title, matching the TUI's precedence
// (crates/dux-tui/src/app/render.rs:500 and sessions.rs:2235): show the
// foreground command if one is running, otherwise the static label.
//
// The foreground command is normalized exactly like the TUI: trim it, strip a
// leading "TERM "/"term " prefix, and ignore it if it ends up empty — in which
// case we fall back to the label.
export function terminalTitle(t: TerminalView): string {
  const raw = t.foreground_cmd
  if (raw != null) {
    // Match the TUI order exactly: trim first, strip a "TERM "/"term " prefix
    // off the trimmed string, then ignore the result only if it is empty/blank.
    const trimmed = raw.trim()
    let cmd = trimmed
    if (trimmed.startsWith("TERM ")) {
      cmd = trimmed.slice("TERM ".length)
    } else if (trimmed.startsWith("term ")) {
      cmd = trimmed.slice("term ".length)
    }
    if (cmd.trim().length > 0) {
      return cmd
    }
  }
  return t.label
}
