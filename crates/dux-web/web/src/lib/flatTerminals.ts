// Pure helpers backing the flat "Terminals" section of the sidebar, shared by the
// desktop sidebar and the mobile hub so the two surfaces never drift. Kept free
// of React so every rule here is trivially unit-testable.
//
// Every session-, project-, and standalone-owned terminal renders in one flat
// "Terminals" section. These helpers assemble its labels and state words.

import { assertNever } from "@/lib/assertNever"
import type { FlatSortKey, StateWord } from "@/lib/flatList"
import {
  ownerKey,
  ownerRefFromWire,
  type TerminalOwnerRef,
} from "@/lib/terminalOwner"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"

// One entry in the flat Terminals section: the terminal, its owner reference (so
// a tap selects/streams it), the owner's display label (the agent name, or the
// project name for a project terminal), the project name tag, and the sibling set
// the same owner shares (so `terminalTitle` can disambiguate two terminals running
// the same app).
export interface FlatTerminal {
  terminal: TerminalView
  owner: TerminalOwnerRef
  ownerLabel: string
  projectName: string
  siblings: readonly TerminalView[]
}

// Decorate the spine's flat `terminals` collection with everything the row needs
// that the terminal itself does not carry: its owner reference, the owner's
// display label, the project tag, and its sibling set.
//
// `terminals` is already flat and owner-tagged. Sessions and projects are lookup
// tables for labels, and the input order is preserved.
//
// A companion terminal is labeled `agent@project` (the agent's display name --
// title, or branch name when untitled -- at its project); a project terminal
// carries just the project name (it has no agent); a standalone terminal carries
// the `~`-shortened directory it opened in (it has no owner to name at all). An owner id that resolves to
// nothing falls back to the id itself, matching the TUI's sidebar: the spine is
// self-consistent so this should not happen, but showing the row with a truthful
// id beats dropping it, which is the silent omission this shape exists to end.
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
        // No owner to name, so the row's second line names the DIRECTORY the
        // terminal opened in, already shortened with `~` by the server. That is
        // also what the sidebar search matches, which is why it goes in
        // `ownerLabel` rather than somewhere beside it. The project tag is
        // empty, truthfully: it belongs to no project.
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

// A terminal's WYSIWYG name-sort key: the same primary label the row shows,
// `foreground_cmd` when present and non-empty else `label`, lowercased. Using the
// displayed label (not the internal `label`) makes name-sort match what the user
// reads. Mirrors the TUI `terminal_items` name key in `app/mod.rs`.
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

// Code-point name comparison, identical in spirit to `sortSessions.ts`'s
// `compareName`: iterate Unicode code points so the order matches Rust's
// `str::cmp` on the lowercased key (the TUI side). Returns <0 / 0 / >0 ascending.
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

// Return the complete displayed order used as the terminal drag baseline. The
// input is already in manual order; computed modes use stable sorting so equal
// keys retain that base order. Comparators mirror the TUI terminal list.
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
