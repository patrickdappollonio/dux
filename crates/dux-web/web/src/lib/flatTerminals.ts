// Pure helpers backing the flat "Terminals" section of the sidebar, shared by the desktop
// sidebar and the mobile hub so the two surfaces never drift. Every session-, project- and
// standalone-owned terminal renders in that one section.

import { assertNever } from "@/lib/assertNever"
import type { FlatSortKey, StateWord } from "@/lib/flatList"
import {
  ownerKey,
  ownerRefFromWire,
  type TerminalOwnerRef,
} from "@/lib/terminalOwner"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"

// One entry in the flat Terminals section. `siblings` is the set the same owner shares,
// so `terminalTitle` can disambiguate two terminals running the same app.
export interface FlatTerminal {
  terminal: TerminalView
  owner: TerminalOwnerRef
  ownerLabel: string
  projectName: string
  siblings: readonly TerminalView[]
}

// Decorate the spine's flat `terminals` with what the row needs and the terminal does not
// carry: owner reference, owner label, project tag, sibling set. The input order is
// preserved, and an owner id that resolves to nothing falls back to the id itself, because
// a row with a truthful id beats a silently dropped row.
export function assembleFlatTerminals(
  terminals: readonly TerminalView[],
  sessions: readonly SessionView[],
  projects: readonly ProjectView[],
  projectName: (id: string) => string,
): FlatTerminal[] {
  const sessionsById = new Map(sessions.map((s) => [s.id, s]))
  const projectsById = new Map(projects.map((p) => [p.id, p]))
  // Group siblings by owner kind and id so equal session/project ids never merge.
  const byOwner = new Map<string, TerminalView[]>()
  for (const terminal of terminals) {
    const key = ownerKey(terminal.owner)
    const group = byOwner.get(key)
    if (group) group.push(terminal)
    else byOwner.set(key, [terminal])
  }

  const out: FlatTerminal[] = []
  for (const terminal of terminals) {
    const wire = terminal.owner
    const siblings = byOwner.get(ownerKey(wire)) ?? [terminal]
    let ownerLabel: string
    let proj: string
    switch (wire.kind) {
      case "session": {
        const session = sessionsById.get(wire.session_id)
        if (session) {
          // A standalone agent belongs to no project, so there is nothing to
          // qualify the label with and the owner is just the agent's name.
          const projectId = workspaceProjectId(session.workspace)
          proj = projectId ? projectName(projectId) : ""
          const label = sessionLabel(session)
          ownerLabel = proj ? `${label}@${proj}` : label
        } else {
          proj = ""
          ownerLabel = wire.session_id
        }
        break
      }
      case "project": {
        proj = projectsById.has(wire.project_id)
          ? projectName(wire.project_id)
          : wire.project_id
        ownerLabel = proj
        break
      }
      case "standalone": {
        // No owner to name, so the second line names the directory the terminal
        // opened in, already `~`-shortened by the server. It goes in `ownerLabel`
        // because that is what the sidebar search matches.
        proj = ""
        ownerLabel = wire.cwd_label
        break
      }
      default:
        return assertNever(wire)
    }
    out.push({
      terminal,
      owner: ownerRefFromWire(wire),
      ownerLabel,
      projectName: proj,
      siblings,
    })
  }
  return out
}

// Mirrors core terminal-row precedence: typing, then running, then idle.
export function terminalStateWord(terminal: TerminalView): StateWord {
  if (terminal.typing) return { label: "Typing", className: "text-dux-typing" }
  if (terminal.working) return { label: "Running", className: "text-green-500" }
  return { label: "Idle", className: "text-muted-foreground" }
}

// The name-sort key is the label the row shows, so the sort matches what the user reads:
// `foreground_cmd` when non-empty else `label`, lowercased. Mirrors TUI `terminal_items`.
function terminalNameKey(t: TerminalView): string {
  const cmd = t.foreground_cmd
  return (cmd && cmd.length > 0 ? cmd : t.label).toLowerCase()
}

// Parse an RFC 3339 timestamp to epoch ms, guarding NaN (an empty/unparseable
// value from an older server sorts as 0). Matches `sortSessions.ts`'s `epoch`.
function terminalEpoch(iso: string): number {
  const ms = Date.parse(iso)
  return Number.isNaN(ms) ? 0 : ms
}

// Iterate Unicode code points so the order matches Rust's `str::cmp` on the lowercased
// key. Returns <0 / 0 / >0 ascending, like `sortSessions.ts`'s `compareName`.
function compareTerminalName(a: TerminalView, b: TerminalView): number {
  const ka = [...terminalNameKey(a)]
  const kb = [...terminalNameKey(b)]
  const len = Math.min(ka.length, kb.length)
  for (let i = 0; i < len; i++) {
    const ca = ka[i].codePointAt(0) ?? 0
    const cb = kb[i].codePointAt(0) ?? 0
    if (ca !== cb) return ca - cb
  }
  return ka.length - kb.length
}

// The complete displayed order, used as the terminal drag baseline. The input is already
// in manual order and computed modes sort stably, so equal keys retain it.
export function displayedTerminalOrder(
  items: FlatTerminal[],
  key: FlatSortKey,
): string[] {
  return sortFlatTerminals(items, key).map((item) => item.terminal.id)
}

export function sortFlatTerminals(
  items: FlatTerminal[],
  key: FlatSortKey,
): FlatTerminal[] {
  const sorted = items.slice()
  switch (key) {
    case "manual":
      // Base order verbatim.
      break
    case "active": {
      // Stable float: hot terminals first (keeping base order), then the rest.
      const hot: FlatTerminal[] = []
      const rest: FlatTerminal[] = []
      for (const item of sorted) {
        if (item.terminal.working || item.terminal.typing) hot.push(item)
        else rest.push(item)
      }
      return [...hot, ...rest]
    }
    case "updated":
      sorted.sort(
        (a, b) => terminalEpoch(b.terminal.updated_at) - terminalEpoch(a.terminal.updated_at),
      )
      break
    case "created":
      sorted.sort(
        (a, b) => terminalEpoch(b.terminal.created_at) - terminalEpoch(a.terminal.created_at),
      )
      break
    case "name":
      sorted.sort((a, b) => compareTerminalName(a.terminal, b.terminal))
      break
    case "name_desc":
      sorted.sort((a, b) => -compareTerminalName(a.terminal, b.terminal))
      break
  }
  return sorted
}
