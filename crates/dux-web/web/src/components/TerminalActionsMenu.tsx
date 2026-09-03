import { ExternalLink, FileCode2, X } from "lucide-react"

import {
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import { editorRootForTarget } from "@/lib/editorRoot"
import { openDeleteTerminal, openEditor, standaloneEditorHash } from "@/lib/store"
import type { TerminalOwnerRef } from "@/lib/store"

// THE TERMINAL'S OWN ACTIONS, the twin of `AgentActionsMenu`.
//
// It is never rendered on its own: `TerminalPaneMenuBody` is the menu, and this
// is the group inside it that is about the terminal. The pane's INPUT group is
// therefore not here but in the wrapper, once per menu, above these rows.
//
// Streaming the terminal is the row's own click, so it is deliberately not
// repeated here (a menu duplicate, "Stream", was removed as misleading). What
// it carries is the two editor entries, matching the agent menu's pair exactly,
// and Close, which stays in the menu rather than becoming an inline X so the
// destructive action keeps its confirm flow and its misclick-safe treatment.
export function TerminalActionsMenu({
  terminalId,
  owner,
}: {
  terminalId: string
  owner: TerminalOwnerRef
}) {
  // The editor's root is the directory this terminal was SPAWNED in, and a
  // terminal owned by an agent is sent to that agent's editor instead: same
  // worktree, and the agent's editor is the one with the git surface.
  // `editorRootForTarget` is what decides that.
  const editorRoot = editorRootForTarget({ kind: "terminal", terminalId, owner })
  return (
    <DropdownMenuGroup>
      {/* The in-app overlay is desktop-only, so on a phone its item would be a
          dead no-op and the row is hidden rather than offered. */}
      <DropdownMenuItem
        className="max-md:hidden"
        onClick={() => openEditor(editorRoot)}
      >
        <FileCode2 />
        Open editor here
      </DropdownMenuItem>
      {/* A real anchor, so a long-press keeps its native open-in-new-tab. */}
      <DropdownMenuItem
        render={
          <a
            href={standaloneEditorHash(editorRoot)}
            target="_blank"
            rel="noopener"
          />
        }
      >
        <ExternalLink />
        Open editor in new tab
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      {/* Neutral color per the destructive convention: the `…` plus
          ConfirmDeleteTerminalDialog are the danger signal. */}
      <DropdownMenuItem onClick={() => openDeleteTerminal(terminalId)}>
        <X />
        Close…
      </DropdownMenuItem>
    </DropdownMenuGroup>
  )
}
