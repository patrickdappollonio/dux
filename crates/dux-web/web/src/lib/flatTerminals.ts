// Pure helpers backing a terminal's row in the agents list, shared by the desktop sidebar,
// the collapsed rail and the mobile hub so the surfaces never drift. Where a row sits is
// `lib/flatList.ts`'s business; this decorates a terminal with what its row shows.

import { assertNever } from "@/lib/assertNever"
import type { StateWord } from "@/lib/flatList"
import {
  ownerKey,
  ownerRefFromWire,
  type TerminalOwnerRef,
} from "@/lib/terminalOwner"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"

// One terminal decorated for its row. `siblings` is the set the same owner shares,
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
  if (terminal.working) return { label: "Running", className: "text-dux-working" }
  return { label: "Idle", className: "text-muted-foreground" }
}
