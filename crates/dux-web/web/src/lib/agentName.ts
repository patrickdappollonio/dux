// Agent-name input rules, ported from `dux_core::git` so the web new-agent
// dialog accepts exactly the same strings the TUI does. The TUI rejects an
// offending keystroke (`agent_name_char_map`); a React input hands over the
// whole next value, so `sanitizeAgentName` maps the string through rules that
// converge on the same accepted set:
//   - space -> dash
//   - drop chars outside [A-Za-z0-9-_/]
//   - first char must be alphanumeric (drop leading -, _, / until one)
//   - collapse "//" to "/"
// A single trailing `/` is deliberately kept, since the user may be mid-typing
// "feat/" before "feat/x"; `isValidAgentName` rejects it at submit, as the
// TUI's `is_valid_agent_name` does.

const ALLOWED = /[A-Za-z0-9\-_/]/

/**
 * Map a full input value through the agent-name character rules listed above.
 * A trailing slash survives here and is rejected by `isValidAgentName`.
 */
export function sanitizeAgentName(next: string): string {
  let out = ""
  for (const raw of next) {
    const ch = raw === " " ? "-" : raw
    if (!ALLOWED.test(ch)) continue
    // First accepted character must be alphanumeric (reject leading -, _, /).
    if (out.length === 0 && !/[A-Za-z0-9]/.test(ch)) continue
    // Collapse "//": skip a slash that would immediately follow a slash.
    if (ch === "/" && out.endsWith("/")) continue
    out += ch
  }
  return out
}

/**
 * Exact port of `dux_core::git::is_valid_agent_name`: non-empty; doesn't start
 * with `-` or `/`; doesn't end with `/`; no `//`; only ASCII alphanumerics,
 * `-`, `_`, `/`. Used to gate the Create button.
 */
export function isValidAgentName(name: string): boolean {
  if (name.length === 0) return false
  if (name.startsWith("-") || name.startsWith("/") || name.endsWith("/")) {
    return false
  }
  if (name.includes("//")) return false
  for (const ch of name) {
    if (!/[A-Za-z0-9\-_/]/.test(ch)) return false
  }
  return true
}
