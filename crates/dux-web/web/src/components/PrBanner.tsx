import { ChevronLeft, ChevronRight, Ellipsis, GitPullRequest } from "lucide-react"

import { PrListMenuItems } from "@/components/PrListMenu"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  prBannerClass,
  prBannerLinkHoverClass,
  prCursorIndex,
  prKeyOf,
  prName,
  prStateLabel,
  stepPrIndex,
} from "@/lib/pr"
import { cn } from "@/lib/utils"
import type { PrKeyView, PrView } from "@/lib/types"

// The strip's one height token. The counter and the three controls carry it
// too, so they sit flush with the strip and with each other; on a coarse
// pointer the whole strip grows to the 40px touch floor with them.
const STRIP_HEIGHT = "h-9 pointer-coarse:h-10"
// The trailing controls' width: the touch floor on a coarse pointer. On a fine
// pointer they keep the strip's 36px, which is clear of the floor's purpose:
// above and below them is the terminal or the strip's own edge, nothing a
// stray press could land on, and beside them sit the anchor (the strip's own
// link) on one side and the strip's edge on the other.
const CONTROL_SIZE = "w-9 pointer-coarse:w-10"

// A slim, one-line PR info strip mirroring the TUI's PR banner lane
// (`render_pr_banner`): a state-colored surface carrying the PR icon, its
// name, the state word, and the (truncated) title of the pull request at the
// stepping cursor. Everything but the trailing controls is one anchor through
// to that pull request, keyboard-focusable like any link. The controls are the
// "n of m" counter and the ‹ › stepper (with two or more), and the list `⋯`
// (always, since a surface's `⋯` never depends on what the surface holds).
//
// `position` is where the strip sits relative to the terminal (top = above,
// bottom = below). It draws a border only on the edge FACING the terminal: the
// opposite edge already abuts a bordered element (the header above / the status
// bar below), so a two-sided border would read as a doubled line.
export function PrBanner({
  prs,
  cursor,
  onCursor,
  onManage,
  position = "top",
}: {
  /** The agent's shown pull requests, in its order; never empty. */
  prs: PrView[]
  /** The pull request the user stepped to, or null for the first. */
  cursor: PrKeyView | null
  onCursor: (key: PrKeyView) => void
  /** Opens the Pull requests dialog. */
  onManage: () => void
  position?: "top" | "bottom"
}) {
  const index = prCursorIndex(prs, cursor)
  const pr = prs[index]
  const state = prStateLabel(pr.state)
  const step = (dir: 1 | -1) => {
    const key = prKeyOf(prs[stepPrIndex(prs.length, index, dir)])
    if (key) onCursor(key)
  }
  const control = cn("rounded-none text-current", STRIP_HEIGHT, CONTROL_SIZE)
  return (
    <div
      data-testid="pr-banner"
      className={cn(
        "flex shrink-0 items-center text-sm",
        STRIP_HEIGHT,
        position === "top" ? "border-b" : "border-t",
        prBannerClass(pr.state),
      )}
    >
      <a
        href={pr.url}
        target="_blank"
        rel="noopener noreferrer"
        className={cn(
          "flex h-full min-w-0 flex-1 items-center gap-2 px-3 transition-colors",
          "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          prBannerLinkHoverClass(pr.state),
        )}
      >
        <GitPullRequest className="size-4 shrink-0" />
        {/* Single font (sans) across the strip: a mono `#N` next to the sans
            state/title misaligned vertically under items-center. */}
        <span className="shrink-0 font-semibold">{prName(pr)}</span>
        <span className="shrink-0 capitalize opacity-80">{state}</span>
        {/* Only the title carries a tooltip: the name is already fully visible
            so repeating it would be redundant. The title itself is `truncate`
            and can clip at narrow widths, so it always gets the full text on
            hover. */}
        <SimpleTooltip content={pr.title}>
          <span className="truncate text-foreground/80">{pr.title}</span>
        </SimpleTooltip>
      </a>
      <div className="flex shrink-0 items-center">
        {prs.length > 1 ? (
          <>
            <span
              className={cn(
                "flex items-center px-2 text-xs tabular-nums opacity-80",
                STRIP_HEIGHT,
              )}
            >
              {index + 1} of {prs.length}
            </span>
            <Button
              variant="ghost"
              size="icon"
              aria-label="Previous pull request"
              className={control}
              onClick={() => step(-1)}
            >
              <ChevronLeft />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              aria-label="Next pull request"
              className={control}
              onClick={() => step(1)}
            >
              <ChevronRight />
            </Button>
          </>
        ) : null}
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                aria-label="All pull requests"
                className={control}
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            side={position === "top" ? "bottom" : "top"}
            className="w-96 max-w-[calc(100vw-2rem)]"
          >
            <PrListMenuItems prs={prs} onManage={onManage} />
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  )
}
