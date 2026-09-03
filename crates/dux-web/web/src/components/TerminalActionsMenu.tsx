import { ExternalLink, FileCode2, X } from "lucide-react"

import {
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import { editorRootForTarget } from "@/lib/editorRoot"
import { openDeleteTerminal, openEditor, standaloneEditorHash } from "@/lib/store"
import type { TerminalOwnerRef } from "@/lib/store"

// THE TERMINAL'S OWN ACTIONS, the twin of `AgentActionsMenu`.
//
// It is never rendered on its own: `PaneMenuBody` is the menu, and this is the
// group inside it that is about the terminal. The pane's INPUT group is
// therefore not here but in the wrapper, once per menu, above these rows.
//
// It appears twice over in that wrapper: as the whole body of a project or
// standalone terminal's menu, and as a labelled group beneath an agent's actions
// when the pane on screen is one of that agent's companion terminals.
//
// Streaming the terminal is the row's own click, so it is deliberately not
// repeated here (a menu duplicate, "Stream", was removed as misleading). What
// it carries is the two editor entries, matching the agent menu's pair exactly,
// and Close, which stays in the menu rather than becoming an inline X so the
// destructive action keeps its confirm flow and its misclick-safe treatment.
export function TerminalActionsMenu({
  terminalId,
  owner,
  label,
}: {
  terminalId: string
  owner: TerminalOwnerRef
  /// A heading over the group, for the one menu that carries these rows beside
  /// somebody else's: a companion terminal's pane wears its agent's menu, and
  /// an unlabelled Close… under an agent's actions reads as closing the agent.
  /// The whole menu is the terminal's everywhere else, so it has no label
  /// there: a heading over every row in a menu says nothing.
  label?: string
}) {
  // The editor's root is the directory this terminal was SPAWNED in, and a
  // terminal owned by an agent is sent to that agent's editor instead: same
  // worktree, and the agent's editor is the one with the git surface.
  // `editorRootForTarget` is what decides that.
  const editorRoot = editorRootForTarget({ kind: "terminal", terminalId, owner })
  return (
    <DropdownMenuGroup>
      {/* Inside the group rather than above it: the primitive's label part reads
          its group from context, and the grouping is also what a screen reader
          announces the heading as. */}
      {label ? <DropdownMenuLabel>{label}</DropdownMenuLabel> : null}
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
