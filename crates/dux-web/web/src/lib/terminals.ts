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

// The terminal's number, parsed from its "Terminal N" label. Used only to
// disambiguate two terminals running the same app (see terminalTitle). Returns
// null for a label that carries no trailing number, which never happens for
// engine-assigned labels but keeps the helper total.
function terminalNumber(label: string): number | null {
  const match = /(\d+)\s*$/.exec(label)
  return match ? Number(match[1]) : null
}

// The terminal's display title. When an app is running in the foreground its
// command name is the most useful label, so we surface it alone ("vim",
// "htop") rather than appending the redundant "Terminal N" suffix. The stable
// label returns the moment the app exits. The one exception is collision: when
// another terminal in `siblings` runs the same app, both would read identically,
// so we disambiguate with the terminal's own counter number ("vim (#1)",
// "vim (#2)"). `siblings` is the set of terminals shown together (one session's
// terminals on the web); it includes `t` itself, which we skip by id. The TUI
// left pane applies the identical rule (crates/dux-tui/src/app/render.rs).
export function terminalTitle(
  t: TerminalView,
  siblings: readonly TerminalView[],
): string {
  const cmd = terminalForeground(t)
  if (cmd == null) return t.label
  const collision = siblings.some(
    (other) => other.id !== t.id && terminalForeground(other) === cmd,
  )
  if (!collision) return cmd
  const n = terminalNumber(t.label)
  return n != null ? `${cmd} (#${n})` : `${cmd} (${t.label})`
}
