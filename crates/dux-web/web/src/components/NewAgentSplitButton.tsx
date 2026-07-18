import { Bot, EllipsisVertical, FolderGit2, GitPullRequest, Plus } from "lucide-react"

import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { openNewAgentPicker, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"

// The New-agent control: the one-click primary opens the picker to create a plain
// agent, and the attached ⋯ segment offers the creation variants. Each variant
// opens the same picker armed with an intent, so the user first picks the target
// project and the from-PR / from-worktree dialog takes over from there (those
// flows are per-project, hence the project pick). Mirrors AddProjectSplitButton:
// same seam-rounding fix, same touch sizing. The from-PR item is hidden when
// GitHub / `gh` is unavailable, matching the per-project ⋯ menu.
export function NewAgentSplitButton({ className }: { className?: string }) {
  const { bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false
  return (
    // [&>button:last-of-type]:rounded-r-lg keeps the trigger's right corners
    // rounded while the menu is open (base-ui's hidden focus-guard span would
    // otherwise steal :last-child and let the group seam square the corner).
    <ButtonGroup
      className={cn("[&>button:last-of-type]:rounded-r-lg", className)}
    >
      <Button
        size="sm"
        className="flex-1 max-md:min-h-11"
        onClick={() => openNewAgentPicker("new")}
      >
        <Plus />
        New agent
      </Button>
      <DropdownMenu>
        {/* Open-state styling keys off data-popup-open (base-ui does not flip
            aria-expanded on an open menu trigger). */}
        <DropdownMenuTrigger
          render={
            <Button
              size="sm"
              aria-label="More ways to create an agent"
              className="min-w-8 max-md:min-h-11 max-md:min-w-11"
            >
              <EllipsisVertical />
            </Button>
          }
        />
        <DropdownMenuContent align="end" side="top">
          <DropdownMenuItem onClick={() => openNewAgentPicker("new")}>
            <Bot />
            New agent…
          </DropdownMenuItem>
          {ghAvailable && (
            <DropdownMenuItem onClick={() => openNewAgentPicker("from_pr")}>
              <GitPullRequest />
              New agent from PR…
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onClick={() => openNewAgentPicker("from_worktree")}>
            <FolderGit2 />
            New agent from existing worktree…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </ButtonGroup>
  )
}
