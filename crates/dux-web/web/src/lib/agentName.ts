// Agent-name input rules, ported from `dux_core::git` so the web new-agent
// dialog accepts exactly the same strings the TUI does.
//
// The TUI filters input per-keystroke via `agent_name_char_map`: it REJECTS a
// keystroke that would make the name invalid (a disallowed char, a leading
// non-alphanumeric, a `/` adjacent to another `/`), and transparently maps a
// space to a dash. React inputs hand us the WHOLE next value instead of a single
// keystroke, so `sanitizeAgentName` maps the full string through the equivalent
// rules. The two converge on identical ACCEPTED strings:
//   - space -> dash                (same)
//   - drop chars outside [A-Za-z0-9-_/]  (TUI rejects the keystroke; we drop it)
//   - first char must be alphanumeric    (TUI rejects a leading -, _, /; we drop
//                                         leading -, _, / until the first alnum)
//   - no "//"                       (TUI rejects the second /; we collapse // -> /)
// One intentional difference: `sanitizeAgentName` does NOT strip a single
// trailing `/`, because the user may be mid-typing (e.g. "feat/" before
// "feat/x"). `isValidAgentName` still rejects a trailing `/` for SUBMIT, which
// is what the TUI's submit-time validation (`is_valid_agent_name`) also does.

const ALLOWED = /[A-Za-z0-9\-_/]/

/**
 * Map a full input value through the agent-name character rules. Spaces become
 * dashes, disallowed characters are dropped, leading non-alphanumerics are
 * dropped until the first alphanumeric, and consecutive slashes are collapsed to
 * a single slash. A single trailing slash is preserved so the user can keep
 * typing a path-style name; submit-time validation (`isValidAgentName`) is what
 * rejects a trailing slash.
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
