import { GitPullRequest } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { prBannerClass, prStateLabel } from "@/lib/pr"
import { cn } from "@/lib/utils"
import type { PrView } from "@/lib/types"

// A slim, one-line PR info strip mirroring the TUI's PR banner lane
// (`render_pr_banner`): a state-colored surface carrying the PR icon, `#N`, the
// state word, and the (truncated) title. The whole strip is one anchor through
// to the PR, keyboard-focusable like any link.
//
// Placement (above vs below the terminal on desktop, always-top on mobile) is
// the caller's responsibility; this component is placement-agnostic.
export function PrBanner({ pr }: { pr: PrView }) {
  const state = prStateLabel(pr.state)
  return (
    <SimpleTooltip content={`#${pr.number} · ${pr.title}`}>
      <a
        href={pr.url}
        target="_blank"
        rel="noopener noreferrer"
        className={cn(
          "flex h-9 shrink-0 items-center gap-2 border-y px-3 text-sm transition-colors",
          "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          prBannerClass(pr.state)
        )}
      >
        <GitPullRequest className="size-4 shrink-0" />
        <span className="shrink-0 font-mono font-semibold">#{pr.number}</span>
        <span className="shrink-0 capitalize opacity-80">{state}</span>
        <span className="truncate text-foreground/80">{pr.title}</span>
      </a>
    </SimpleTooltip>
  )
}
