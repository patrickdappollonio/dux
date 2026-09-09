import { EllipsisVertical, Plus, SquarePlus } from "lucide-react"

import { CreationOverflowMenuItems } from "@/components/CreationOverflowMenuItems"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { launcherVerb } from "@/lib/launcherVerb"
import { openAddProject, openNewAgentPicker, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"

// The launcher corner on every surface: the desktop sidebar's footer and the
// mobile hub's bottom bar render this one component. One filled verb, one square
// ⋯, and that is the whole corner.
//
// The verb is the workspace's one primary creation click: "New agent" whenever a
// project exists, flipping to "Add project" on a confirmed-empty workspace,
// where a new-agent picker would open with nothing to pick. The decision is the
// shared pure helper (lib/launcherVerb.ts), which the empty list's hero button
// reads too, so the two can never name different next steps.
//
// The ⋯ does not flip: it is the constant home of every other way to create
// something, grouped under Agents, Terminals and Projects, so its rows never
// move under the cursor.
export function LauncherCorner({ className }: { className?: string }) {
  const { spine } = useDux()
  // null while the spine has not arrived: see launcherVerb for why that is not
  // a zero.
  const verb = launcherVerb(spine ? spine.projects.length : null)
  const addProject = verb === "add-project"
  return (
    // Two separate rounded buttons with a gap, NOT a seam-joined ButtonGroup:
    // the ⋯ is the corner's overflow rather than a second half of the verb, and
    // the gap is also what keeps a mistimed tap off the wrong control.
    <div className={cn("flex items-center gap-2", className)}>
      {/* size="sm" is the shared height token for both controls, with
        * max-md:min-h-11 lifting each over the touch floor. The desktop height
        * is the per-axis exemption: with a mouse the only neighbour on either
        * axis is the ⋯, which opens a menu and executes nothing. */}
      <Button
        size="sm"
        className="min-w-0 flex-1 max-md:min-h-11"
        onClick={addProject ? openAddProject : () => openNewAgentPicker("new")}
      >
        {addProject ? <SquarePlus /> : <Plus />}
        <span className="truncate">
          {addProject ? "Add project" : "New agent"}
        </span>
      </Button>
      <DropdownMenu>
        {/* Outline, deliberately quieter than the filled verb: this corner draws
          * exactly one primary and the ⋯ only reveals a menu, the reasoning that
          * also keeps the Agents header's sort trigger quieter than its +.
          * Outline carries the data-popup-open tint too, since base-ui does not
          * flip aria-expanded on an open menu trigger. The aria-label names
          * every group behind it, not just the agents. */}
        <DropdownMenuTrigger
          render={
            <Button
              size="sm"
              variant="outline"
              aria-label="More ways to create"
              className="min-w-7 shrink-0 px-0 max-md:min-h-11 max-md:min-w-11"
            >
              <EllipsisVertical />
            </Button>
          }
        />
        <DropdownMenuContent align="end" side="top">
          <CreationOverflowMenuItems />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}
