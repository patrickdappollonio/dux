// Pure helpers backing the flat "Terminals" section of the sidebar, shared by the
// desktop sidebar and the mobile hub so the two surfaces never drift. Kept free
// of React so every rule here is trivially unit-testable.
//
// Since the terminal/agent sidebar parity work, companion terminals are no longer
// nested under their agent row and project terminals are no longer a loose group:
// EVERY terminal (session-owned + project-owned) renders flat in one "Terminals"
// section at the bottom of the list. These helpers own the two data decisions that
// section needs: assembling that flat list with each terminal's owner label, and
// deriving a terminal's state word from its working/typing flags.

import type { StateWord } from "@/lib/flatList"
// Type-only import: erased at compile time, so this does not create a runtime
// import cycle with the store (which imports many lib modules).
import type { TerminalOwnerRef } from "@/lib/store"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"

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

// Assemble every terminal into one flat list in stable spine order: each session's
// companion terminals (in session order) first, then each project's terminals (in
// the given project display order). Session terminals are labeled with the agent's
// display name (title, or branch name when untitled); project terminals with the
// project name. `projects` must already be in display order; `projectName` resolves
// a project id to its display name (with the orphan fallback).
export function assembleFlatTerminals(
  sessions: readonly SessionView[],
  projects: readonly ProjectView[],
  projectName: (id: string) => string,
): FlatTerminal[] {
  const out: FlatTerminal[] = []
  for (const session of sessions) {
    const ownerLabel = session.title || session.branch_name
    // Defensive `?? []`: the spine normalizes `terminals` at ingestion, but this
    // helper is also fed directly in tests and stays total if a caller omits it.
    const terminals = session.terminals ?? []
    for (const terminal of terminals) {
      out.push({
        terminal,
        owner: { kind: "session", sessionId: session.id },
        ownerLabel,
        projectName: projectName(session.project_id),
        siblings: terminals,
      })
    }
  }
  for (const project of projects) {
    const projectTerminals = project.terminals ?? []
    for (const terminal of projectTerminals) {
      out.push({
        terminal,
        owner: { kind: "project", projectId: project.id },
        ownerLabel: projectName(project.id),
        projectName: projectName(project.id),
        siblings: projectTerminals,
      })
    }
  }
  return out
}

// A terminal's colored state word, mirroring the agent row's `stateWord` but with
// only the three states a terminal can have (no detached/exited/attention):
// typing outranks working outranks idle, matching the TUI's priority. Colors reuse
// the exact tokens the agent word uses so the two never drift: the soft-violet
// typing token, the active-green working color, muted for idle.
export function terminalStateWord(terminal: TerminalView): StateWord {
  if (terminal.typing) return { label: "Typing", className: "text-dux-typing" }
  if (terminal.working) return { label: "Working", className: "text-green-500" }
  return { label: "Idle", className: "text-muted-foreground" }
}
