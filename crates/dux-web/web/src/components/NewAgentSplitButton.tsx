import { EllipsisVertical, Plus } from "lucide-react"

import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { newAgentMenuItems } from "@/lib/creationMenus"
import { openNewAgentPicker, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"

// The New-agent control: the one-click primary opens the picker to create a plain
// agent, and the attached ⋯ segment offers the creation variants, rendered from
// the shared list in creationMenus.ts (also the cog app menu's "New agent"
// submenu, so the two surfaces cannot drift). See creationMenus.ts for what
// each variant opens and why. Mirrors AddProjectSplitButton: same seam-rounding
// fix, same touch sizing. The from-PR item is hidden when GitHub / `gh` is
// unavailable, matching the per-project ⋯ menu.
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
          {newAgentMenuItems({ ghAvailable }).map((item) => {
            const Icon = item.icon
            return (
              <DropdownMenuItem key={item.id} onClick={item.run}>
                <Icon />
                {item.title}
              </DropdownMenuItem>
            )
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    </ButtonGroup>
  )
}
