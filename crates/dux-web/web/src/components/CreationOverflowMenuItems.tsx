import { Fragment } from "react"

import {
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import {
  addProjectMenuItems,
  newMenuItems,
  splitCreationGroups,
  NEW_AGENT_PLAIN_ID,
  type CreationMenuAction,
} from "@/lib/creationMenus"
import { useDux } from "@/lib/store"

/**
 * The body of the launcher's ⋯ menu: every way to create something that is not
 * the corner's own filled verb, under three headings. One component, every
 * surface that carries the ⋯ (the sidebar footer's corner, the mobile hub's,
 * and the collapsed icon rail's), so those menus cannot drift; the caller
 * supplies its own <DropdownMenuContent> wrapper, exactly as ProjectMenuItems
 * and InputMenuItems do.
 *
 * The rows are the shared lists from creationMenus.ts verbatim, minus the plain
 * agent (the verb beside the ⋯ already is that click; see NEW_AGENT_PLAIN_ID).
 * The GROUPING is presentation added here and nowhere else: the cog app menu
 * renders the same lists unlabeled, because its submenu titles already name the
 * group.
 *
 * Each heading is a DropdownMenuLabel INSIDE a DropdownMenuGroup: base-ui's
 * GroupLabel throws outside a Menu.Group, so the parent is load-bearing, not
 * decoration. Label rows are non-interactive, so the touch floor does not apply
 * to them; on a phone the whole menu renders as a bottom sheet and the headings
 * come along.
 */
export function CreationOverflowMenuItems() {
  const { bootstrap } = useDux()
  const ghAvailable = bootstrap?.gh_available ?? false
  // Partition rather than two id filters: the rule in the shared list is what
  // decides where agents stop and terminals start (see splitCreationGroups).
  const [agentItems = [], terminalItems = []] = splitCreationGroups(
    newMenuItems({ ghAvailable }).filter(
      (entry) => entry.id !== NEW_AGENT_PLAIN_ID,
    ),
  )
  const groups: { id: string; label: string; items: CreationMenuAction[] }[] = [
    { id: "agents", label: "Agents", items: agentItems },
    { id: "terminals", label: "Terminals", items: terminalItems },
    {
      id: "projects",
      label: "Projects",
      // The project variants are their own list, appended here rather than
      // spliced into the New one: the cog keeps them in a separate submenu.
      items: addProjectMenuItems().filter(
        (entry): entry is CreationMenuAction => entry.kind === "item",
      ),
    },
  ]
  // A heading with nothing under it is a dangling word, and a rule with nothing
  // on one side is a dangling line: drop empty groups first, then draw the rule
  // BETWEEN what survives. That is what keeps the separators off both edges no
  // matter which gate hid what.
  const drawn = groups.filter((group) => group.items.length > 0)
  return (
    <>
      {drawn.map((group, index) => (
        // Fragment, not a wrapper element: base-ui walks the popup's own
        // children to drive roving focus, so an extra box between them would
        // cost the arrow keys.
        <Fragment key={group.id}>
          {index > 0 ? <DropdownMenuSeparator /> : null}
          <DropdownMenuGroup>
            <DropdownMenuLabel>{group.label}</DropdownMenuLabel>
            {group.items.map((entry) => {
              const Icon = entry.icon
              return (
                <DropdownMenuItem key={entry.id} onClick={entry.run}>
                  <Icon />
                  {entry.title}
                </DropdownMenuItem>
              )
            })}
          </DropdownMenuGroup>
        </Fragment>
      ))}
    </>
  )
}
