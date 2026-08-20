// How the standalone editor tab's header names what the editor is rooted at.
//
// Pure, and drawn from the same facts the sidebar row is drawn from, so the tab
// and the row cannot end up calling the same thing two different names. A
// terminal is named the way its row names it: the terminal's own identity, with
// the second line's owner text beside it, which for a standalone terminal is
// the home-collapsed directory it opened in.

import { matchWireOwner } from "./terminalOwner"
import { sessionLabel } from "./agentWorkspace"
import { terminalTitle } from "./terminals"
import type { EditorRoot } from "./editorRoot"
import type { Spine } from "./workspaceApi"

export interface StandaloneEditorName {
  // Which glyph the header wears. A terminal-rooted tab must read as one at a
  // glance rather than as an agent whose name happens to look like a path.
  glyph: "agent" | "terminal"
  name: string
  // The quieter half: a terminal's owner or spawn directory. Agents carry none,
  // because their name is already the whole identity.
  detail: string | null
}

export function standaloneEditorName(
  root: EditorRoot | null,
  spine: Spine | null,
): StandaloneEditorName {
  if (root === null) return { glyph: "agent", name: "", detail: null }
  if (root.kind === "agent") {
    const session = spine?.sessions.find((s) => s.id === root.sessionId)
    return {
      glyph: "agent",
      // Falling back to the raw id rather than to nothing: an id is poor but
      // truthful, and an empty header says the tab is broken when it is not.
      // sessionLabel is workspace-aware: a standalone agent has no branch to
      // fall back to, so its label falls through to its folder's name.
      name: session ? sessionLabel(session) : root.sessionId,
      detail: null,
    }
  }
  const terminal = spine?.terminals.find((t) => t.id === root.terminalId)
  if (!terminal) {
    return { glyph: "terminal", name: root.terminalId, detail: null }
  }
  const siblings = spine?.terminals ?? []
  const detail = matchWireOwner<string | null>(terminal.owner, {
    session: (owner) => {
      const session = spine?.sessions.find((s) => s.id === owner.session_id)
      return session ? sessionLabel(session) : owner.session_id
    },
    project: (owner) => {
      const project = spine?.projects.find((p) => p.id === owner.project_id)
      return project ? project.name : owner.project_id
    },
    // No owner to name, so the row's second line names the DIRECTORY the
    // terminal opened in, already collapsed to `~` by the server. That is also
    // the root this editor is pinned to.
    standalone: (owner) => owner.cwd_label,
  })
  return {
    glyph: "terminal",
    name: terminalTitle(terminal, siblings),
    detail,
  }
}
