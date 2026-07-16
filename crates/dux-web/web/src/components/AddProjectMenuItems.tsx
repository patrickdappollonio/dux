import { FolderGit2, FolderPlus } from "lucide-react"

import { DropdownMenuItem } from "@/components/ui/dropdown-menu"
import { openAddProject, openAddProjectForInit } from "@/lib/store"

/**
 * The shared body of the add-project split button's dropdown, rendered by both
 * the desktop sidebar footer and the mobile hub so the two menus never drift
 * (the same one-definition rule as ProjectMenuItems). The caller supplies its
 * own <DropdownMenuContent> wrapper. Menu tenets: every item keeps a leading
 * lucide icon, neutral color, and a trailing "…" because both open a dialog.
 *
 * Both items open the SAME picker; the intent only changes a header hint, and
 * the primary-action ladder decides the real action from the server's
 * inspection, so "Initialize a repository…" stays discoverable in the menu
 * while remaining reachable through the plain picker too.
 */
export function AddProjectMenuItems() {
  return (
    <>
      <DropdownMenuItem onClick={openAddProject}>
        <FolderGit2 />
        Add project…
      </DropdownMenuItem>
      <DropdownMenuItem onClick={openAddProjectForInit}>
        <FolderPlus />
        Initialize a repository…
      </DropdownMenuItem>
    </>
  )
}
