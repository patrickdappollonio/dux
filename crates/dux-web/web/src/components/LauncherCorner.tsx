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

// THE launcher corner, on every surface: the desktop sidebar's footer and the
// mobile hub's bottom bar render this one component (the two split buttons it
// replaces had already drifted once between the surfaces, which is the drift
// this shape exists to prevent). One filled verb, one square ⋯, and that is the
// whole corner.
//
// The verb is the workspace's ONE primary creation click. It is "New agent"
// whenever a project exists, and flips to "Add project" on a confirmed-empty
// workspace, where a new-agent picker would open with nothing to pick. The
// decision is the shared pure helper (lib/launcherVerb.ts), which the empty
// list's hero button reads too, so the two can never disagree about what the
// next click is.
//
// The ⋯ does NOT flip with it: it is the constant home of every other way to
// create something (CreationOverflowMenuItems, grouped under Agents /
// Terminals / Projects), so its rows never move under the cursor.
//
// Variant: verb and ⋯ are one action cluster and share ONE variant, the filled
// default, per CLAUDE.md. What separates the two controls is width and glyph,
// not weight.
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
      {/* size="sm" (h-7) is the shared height token for both controls;
          max-md:min-h-11 lifts each over the 44px touch floor where a finger is
          the pointer. The desktop h-7 is the per-axis exemption: with a mouse
          the only neighbour on either axis is the ⋯, which opens a menu and
          executes nothing. */}
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
        {/* OUTLINE, deliberately quieter than the filled verb: the approved
            design draws exactly one primary in this corner, and the ⋯ only
            reveals a menu, the same reasoning that keeps the Agents header's
            Sort trigger quieter than its + (a menu-revealer is not an
            act-button). Outline also carries the data-popup-open open-state
            tint (base-ui does not flip aria-expanded on an open menu trigger).
            The aria-label names every group behind it, not just the agents. */}
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
