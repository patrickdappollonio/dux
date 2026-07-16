import { EllipsisVertical, Plus } from "lucide-react"

import { AddProjectMenuItems } from "@/components/AddProjectMenuItems"
import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { openAddProject } from "@/lib/store"

// The mobile hub's add-project split button: the full-width primary keeps
// today's one-tap "Add project", and the attached ⋯ segment (min 44px both
// ways, above the 40px touch floor) opens the shared add-variants menu. Both
// hub entries (populated list and empty state) render this same composition,
// and the menu body is the shared AddProjectMenuItems so mobile and desktop
// cannot drift. Misclick tenet: a stray tap on the ⋯ segment only opens a menu
// (nothing executes), and every item opens the same non-destructive picker.
export function AddProjectSplitButton({ className }: { className?: string }) {
  return (
    <ButtonGroup className={className}>
      <Button
        variant="outline"
        className="min-h-11 flex-1"
        onClick={openAddProject}
      >
        <Plus />
        Add project
      </Button>
      <DropdownMenu>
        {/* Open-state styling keys off data-popup-open (base-ui does not flip
            aria-expanded on an open menu trigger). */}
        <DropdownMenuTrigger
          render={
            <Button
              variant="outline"
              aria-label="More ways to add a project"
              className="min-h-11 min-w-11"
            >
              <EllipsisVertical />
            </Button>
          }
        />
        <DropdownMenuContent align="end" side="top">
          <AddProjectMenuItems />
        </DropdownMenuContent>
      </DropdownMenu>
    </ButtonGroup>
  )
}
