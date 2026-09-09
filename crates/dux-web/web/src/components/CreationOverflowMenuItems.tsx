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
 * the corner's own filled verb, under three headings. One component for every
 * surface carrying the ⋯, so those menus cannot drift; the caller supplies its
 * own content wrapper, as `ProjectMenuItems` and `InputMenuItems` do.
 *
 * The rows are the shared lists from creationMenus.ts verbatim, minus the plain
 * agent, which is the verb beside the ⋯. The grouping is presentation added
 * here alone: the cog app menu renders the same lists unlabeled, its submenu
 * titles already naming the group.
 *
 * Each heading is a `DropdownMenuLabel` inside a `DropdownMenuGroup`, because
 * base-ui's GroupLabel throws outside a Menu.Group. Label rows are
 * non-interactive, so the touch floor does not apply to them.
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
  // A heading with nothing under it is a dangling word and a rule with nothing on
  // one side is a dangling line, so empty groups are dropped first and the rules
  // drawn between what survives.
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
