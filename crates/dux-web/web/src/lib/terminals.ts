import type { TerminalView } from "@/lib/types"

// The terminal's NORMALIZED foreground command, or null when the shell itself
// is in the foreground (idle). Normalization ports the TUI's Kill-Running
// overlay order exactly (crates/dux-tui/src/app/sessions.rs:~2235): trim
// first, strip a leading "TERM "/"term " prefix off the trimmed string, then
// discard the result only if it is empty/blank. (The TUI's left pane renders
// the raw command verbatim — render.rs:~691; we apply the normalization on
// both reads here because it only affects pathological comm names and keeps
// one helper.)
export function terminalForeground(t: TerminalView): string | null {
  const raw = t.foreground_cmd
  if (raw == null) return null
  const trimmed = raw.trim()
  let cmd = trimmed
  if (trimmed.startsWith("TERM ")) {
    cmd = trimmed.slice("TERM ".length)
  } else if (trimmed.startsWith("term ")) {
    cmd = trimmed.slice("term ".length)
  }
  return cmd.trim().length > 0 ? cmd : null
}

// The terminal's display title, mirroring the TUI's LEFT PANE composite
// (crates/dux-tui/src/app/render.rs:~691-702): when a command is running the
// row reads "{cmd} · {label}" so the running process is prominent while the
// terminal's stable identity stays visible; idle terminals show just the
// label.
export function terminalTitle(t: TerminalView): string {
  const cmd = terminalForeground(t)
  return cmd ? `${cmd} · ${t.label}` : t.label
}
