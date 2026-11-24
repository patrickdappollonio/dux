import { ChevronDown, Plus } from "lucide-react"
import { Fragment, useId } from "react"

import { Button } from "@/components/ui/button"
import { ButtonGroup, ButtonGroupSeparator } from "@/components/ui/button-group"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  launcherCornerModel,
  newLauncherModel,
  selectAgentCount,
  selectGhAvailable,
  selectProjectCount,
  type NewLauncherRow,
} from "@/lib/creationMenus"
import { MenuItemReason } from "@/components/MenuReason"
import { useMenuReason, type MenuReasonHandle } from "@/hooks/use-menu-reason"
import { openAgentTerminalPicker, useDuxSelector } from "@/lib/store"
import { cn } from "@/lib/utils"

// Said beneath "New terminal for an agent…" while a confirmed zero agents greys it.
const NO_AGENTS_REASON =
  "There are no agents yet. Create an agent first, then open a terminal for it."

// The launcher corner on every surface: the desktop sidebar's footer and the
// mobile hub's bottom bar render this one component. One filled split button:
// the main half starts a new agent, and the ▾ half opens every other way to
// start something, as a menu (a bottom sheet on a phone).
//
// Both halves come from the New dialog's own groups (`newLauncherModel`), so an
// entry runs exactly what the dialog's copy of it runs, gated the same way.
// It subscribes to the three inputs alone, since it sits on screen all the time.
export function LauncherCorner({ className }: { className?: string }) {
  const ghAvailable = useDuxSelector(selectGhAvailable)
  const projectCount = useDuxSelector(selectProjectCount)
  const agentCount = useDuxSelector(selectAgentCount)
  const { primary, menu } = launcherCornerModel(
    newLauncherModel({ ghAvailable, projectCount, agentCount }),
  )
  // The lead sentence's id, named by every row that is greyed for want of a
  // project, so a screen reader hears it on the row. The lead and those rows
  // both exist exactly when the workspace has no project.
  const leadId = useId()
  const noAgents = useMenuReason(
    menu.some((group) => group.rows.some((row) => row.waitingFor === "agent"))
      ? NO_AGENTS_REASON
      : null,
  )
  return (
    // size="sm" is the one height token for both halves. On a computer they
    // are 28px tall, the per-axis exemption: the corner has no other control
    // above or below it. The ▾ half is 32px wide beside the main half, which a
    // mouse does not miss; where a finger is the pointer (a coarse pointer, or
    // a phone's width) both halves take the 44px floor in height and the ▾ half
    // in width as well, because a stray tap there would open the wrong one.
    <ButtonGroup className={cn("w-full", className)}>
      <Button
        size="sm"
        className="min-w-0 flex-1 max-md:min-h-11 pointer-coarse:min-h-11"
        onClick={() => run(primary)}
      >
        <Plus />
        {primary.menuTitle}
      </Button>
      <ButtonGroupSeparator className="bg-primary-foreground/20" />
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              size="sm"
              aria-label="More ways to start"
              // The chevron points at the menu: down while it is closed and up
              // while it is open, read from the trigger's own open state.
              className="w-8 px-0 max-md:min-h-11 max-md:w-11 pointer-coarse:min-h-11 pointer-coarse:w-11 [&[data-popup-open]>svg]:rotate-180"
            >
              <ChevronDown className="transition-transform motion-reduce:transition-none" />
            </Button>
          }
        />
        <DropdownMenuContent side="top" align="end" className="md:w-[min(23rem,calc(100vw-2rem))]">
          {menu.map((group, index) => (
            <Fragment key={group.id}>
              {index > 0 ? <DropdownMenuSeparator /> : null}
              <DropdownMenuGroup>
                <DropdownMenuLabel>{group.title}</DropdownMenuLabel>
                {group.lead ? (
                  <p id={leadId} className="px-1.5 pb-1 text-xs text-foreground">
                    {group.lead}
                  </p>
                ) : null}
                {group.rows.map((row) => (
                  <Fragment key={row.id}>
                    <LauncherMenuEntry row={row} leadId={leadId} noAgents={noAgents} />
                    {row.waitingFor === "agent" ? <MenuItemReason reason={noAgents} /> : null}
                  </Fragment>
                ))}
              </DropdownMenuGroup>
            </Fragment>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </ButtonGroup>
  )
}

function LauncherMenuEntry({
  row,
  leadId,
  noAgents,
}: {
  row: NewLauncherRow
  leadId: string
  noAgents: MenuReasonHandle
}) {
  const Icon = row.icon
  return (
    <DropdownMenuItem
      data-row-id={row.id}
      disabled={row.disabled}
      aria-describedby={
        row.waitingFor === "project"
          ? leadId
          : row.waitingFor === "agent"
            ? noAgents.describedBy
            : undefined
      }
      onClick={() => run(row)}
      className="items-start gap-2.5 py-1.5"
    >
      <Icon className="mt-0.5 text-muted-foreground" />
      <span className="flex min-w-0 flex-col gap-px">
        <span data-entry-title="" className="font-medium">
          {row.menuTitle}
        </span>
        <span className="text-xs text-muted-foreground">{row.summary}</span>
      </span>
    </DropdownMenuItem>
  )
}

// The agent-picking step is the New dialog's own, opened on its own here.
function run(row: NewLauncherRow) {
  if (row.action.kind === "pick-agent") openAgentTerminalPicker()
  else row.action.run()
}
