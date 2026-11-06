import { ExternalLink, GitBranch, GitPullRequest } from "lucide-react"

import {
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { prIconClass, prName, prOrigin, prStateLabel } from "@/lib/pr"
import { cn } from "@/lib/utils"
import type { PrView } from "@/lib/types"

// The list behind the banner's `⋯` and the phone chip: every shown pull
// request in the agent's order, each a link to it, then the way into the Pull
// requests dialog. One body, so the desktop dropdown and the phone sheet (the
// same primitive at phone width) cannot list them differently.
export function PrListMenuItems({
  prs,
  onManage,
}: {
  prs: PrView[]
  onManage: () => void
}) {
  return (
    <>
      <DropdownMenuGroup>
        <DropdownMenuLabel>
          {formatRegularCount(prs.length, "pull request")} on this agent
        </DropdownMenuLabel>
        {prs.map((pr) => (
          <PrListMenuRow key={pr.url} pr={pr} />
        ))}
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={onManage}>
        <GitPullRequest />
        Pull requests…
      </DropdownMenuItem>
    </>
  )
}

function PrListMenuRow({ pr }: { pr: PrView }) {
  const tone = prIconClass(pr.state)
  const origin = prOrigin(pr)
  return (
    // A real anchor, so middle-click and ctrl/cmd-click keep their native
    // new-tab meaning.
    <DropdownMenuItem
      render={<a href={pr.url} target="_blank" rel="noopener noreferrer" />}
      className="items-center gap-2 py-1.5"
    >
      <GitPullRequest className={tone} />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex min-w-0 items-baseline gap-1.5">
          <span className={cn("shrink-0 font-semibold", tone)}>{prName(pr)}</span>
          <span className="truncate">{pr.title}</span>
        </span>
        <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
          <span className={tone}>{prStateLabel(pr.state)}</span>
          {origin?.kind === "branch" ? (
            <>
              <span aria-hidden>•</span>
              <GitBranch className="size-3" />
              <span className="truncate">{origin.name}</span>
            </>
          ) : origin?.kind === "attached" ? (
            <>
              <span aria-hidden>•</span>
              <span>attached by you</span>
            </>
          ) : null}
        </span>
      </span>
      <ExternalLink className="text-muted-foreground" />
    </DropdownMenuItem>
  )
}
