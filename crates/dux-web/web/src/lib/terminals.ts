import { assertNever } from "@/lib/assertNever"
import {
  ownerKey,
  sameOwner,
  type TerminalOwnerRef,
} from "@/lib/terminalOwner"
import type { TerminalView } from "@/lib/types"

// Every terminal owned by `owner`, in the order the flat collection carries them
// (the global `sort_order` base).
export function terminalsForOwner(
  terminals: readonly TerminalView[],
  owner: TerminalOwnerRef,
): TerminalView[] {
  return terminals.filter((t) => sameOwner(t.owner, owner))
}

// Whether `owner` owns a terminal with `terminalId`. This is the membership test
// the router and the deep-link restores run: a terminal must still exist UNDER
// the owner its address names, so merely existing is not enough.
export function ownerHasTerminal(
  terminals: readonly TerminalView[],
  owner: TerminalOwnerRef,
  terminalId: string,
): boolean {
  return terminals.some((t) => t.id === terminalId && sameOwner(t.owner, owner))
}

// Terminals grouped by `ownerKey`, keeping input order inside each group. The
// TOTAL grouping: `ownerKey` answers for every owner, so no terminal falls out.
// Use it wherever every terminal must be accounted for; the two-bucket
// `groupTerminalsByOwner` below answers a narrower question.
export function groupTerminalsByOwnerKey(
  terminals: readonly TerminalView[],
): Map<string, TerminalView[]> {
  const groups = new Map<string, TerminalView[]>()
  for (const t of terminals) {
    const key = ownerKey(t.owner)
    const group = groups.get(key)
    if (group) group.push(t)
    else groups.set(key, [t])
  }
  return groups
}

// Terminals bucketed into the two owners a PROJECT can reach: its own project
// terminals, and those of the sessions it owns. Each bucket keeps input order,
// the global `sort_order` base.
//
// LOSSY ON PURPOSE, for `projectLiveCounts`, whose whole question is how many
// terminals a project reaches: a terminal owned by neither reaches no project,
// so it belongs in no bucket. Anything that must ACCOUNT for every terminal uses
// `groupTerminalsByOwnerKey`, and anything whose behaviour depends on the owner
// uses `matchOwner`/`matchWireOwner` so a new kind is a compile error.
export interface TerminalsByOwner {
  bySession: Map<string, TerminalView[]>
  byProject: Map<string, TerminalView[]>
}

export function groupTerminalsByOwner(
  terminals: readonly TerminalView[],
): TerminalsByOwner {
  const bySession = new Map<string, TerminalView[]>()
  const byProject = new Map<string, TerminalView[]>()
  const push = (
    map: Map<string, TerminalView[]>,
    key: string,
    t: TerminalView,
  ) => {
    const group = map.get(key)
    if (group) group.push(t)
    else map.set(key, [t])
  }
  for (const t of terminals) {
    const owner = t.owner
    switch (owner.kind) {
      case "session":
        push(bySession, owner.session_id, t)
        break
      case "project":
        push(byProject, owner.project_id, t)
        break
      // A standalone terminal reaches no project, so it belongs in neither
      // bucket. This is the "LOSSY ON PURPOSE" case noted above.
      case "standalone":
        break
      default:
        return assertNever(owner)
    }
  }
  return { bySession, byProject }
}

// The terminal's NORMALIZED foreground command, or null when the shell itself is
// in the foreground. Twin of core's
// `dux_core::terminal_title::terminal_foreground_display`, which owns the
// decision, pinned by shared vectors: trim, then strip a leading "TERM "/"term "
// off the trimmed string, then discard the result only if it is blank.
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

// The terminal's display title: a running foreground app's command name alone
// ("vim"), falling back to the stable label the moment it exits, and
// disambiguated by the terminal's counter number ("vim (#1)") when a sibling
// runs the same app. `siblings` is the set shown together and includes `t`,
// which is skipped by id. Twin of core's
// `dux_core::terminal_title::terminal_title`, which owns the decision and is
// pinned by shared vectors; the core fn is passed the other siblings with self
// already excluded, but the rule is identical.
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
