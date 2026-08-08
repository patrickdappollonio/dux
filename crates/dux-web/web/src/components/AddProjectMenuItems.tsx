import { DropdownMenuItem } from "@/components/ui/dropdown-menu"
import { addProjectMenuItems } from "@/lib/creationMenus"

/**
 * The shared body of the add-project split button's dropdown, rendered by both
 * the desktop sidebar footer and the mobile hub so the two menus never drift
 * (the same one-definition rule as ProjectMenuItems). The caller supplies its
 * own <DropdownMenuContent> wrapper. Menu tenets: every item keeps a leading
 * lucide icon, neutral color, and a trailing "…" because both open a dialog.
 *
 * The items themselves come from the shared list in creationMenus.ts, which
 * the cog app menu's "Add project" submenu also renders, so THOSE two surfaces
 * cannot drift either. See creationMenus.ts for why both variants open the
 * same picker.
 */
export function AddProjectMenuItems() {
  return (
    <>
      {addProjectMenuItems().map((item) => {
        const Icon = item.icon
        return (
          <DropdownMenuItem key={item.id} onClick={item.run}>
            <Icon />
            {item.title}
          </DropdownMenuItem>
        )
      })}
    </>
  )
}
