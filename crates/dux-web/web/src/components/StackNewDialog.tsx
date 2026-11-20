import { Bot, Plus, SquareTerminal } from "lucide-react"
import type { LucideIcon } from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import {
  addToAgentBlockHint,
  useAgentChangeBlock,
} from "@/hooks/use-agent-active-elsewhere"
import { defaultProviderForSession } from "@/lib/agentTabs"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { FOCUS_RING } from "@/lib/focusRing"
import { addTab, createTerminal, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView } from "@/lib/types"

// The desktop pane header's New tab…: the first of the pane's controls, in the
// outline family and at the height token Theater and Macros share, on an agent's
// pane only, since a project or standalone terminal has no agent to add to. It
// asks what to add rather than adding a tab outright, because a terminal in the
// dock is the other half of the same question. When the header folds it into
// the pane's menu, that row opens the same dialog. While this device does not
// drive the agent it is disabled with the reason, as Macros… beside it is.
export function NewTabButton({ session }: { session: SessionView }) {
  const [adding, setAdding] = useState(false)
  const block = useAgentChangeBlock(session)
  // Mounted whatever the button shows, so the dialog hears the block arrive
  // while it is open and closes for good.
  const dialog = (
    <StackNewDialog session={session} open={adding} onClose={() => setAdding(false)} />
  )
  if (block !== null) {
    // A disabled button takes no pointer events, so the hint hangs off a
    // focusable wrapper, which wears the focus ring the button cannot.
    return (
      <>
        <SimpleTooltip content={addToAgentBlockHint(block)}>
          <span
            tabIndex={0}
            data-testid="new-tab-disabled"
            className={cn("inline-flex shrink-0 rounded-lg", FOCUS_RING)}
          >
            <Button
              variant="outline"
              size="default"
              className="shrink-0 pointer-coarse:h-10"
              disabled
            >
              <Plus />
              New tab…
            </Button>
          </span>
        </SimpleTooltip>
        {dialog}
      </>
    )
  }
  return (
    <>
      <SimpleTooltip content="New agent tab or terminal">
        <Button
          variant="outline"
          // `default` like Theater and Macros beside it: one height, and the
          // label changes only the width. 40px tall under a finger, like them.
          size="default"
          className="shrink-0 pointer-coarse:h-10"
          onClick={() => setAdding(true)}
        >
          <Plus />
          New tab…
        </Button>
      </SimpleTooltip>
      {dialog}
    </>
  )
}

// What New tab… asks: another of this agent's tabs, on its default provider,
// or another terminal in its dock. It runs exactly what the agent's own menu
// runs, and another provider stays in that menu's New agent tab….
export function StackNewDialog({
  session,
  open,
  onClose,
}: {
  session: SessionView
  open: boolean
  onClose: () => void
}) {
  const { spine, bootstrap, createTabInFlight } = useDux()
  // The take-over card coming up closes the dialog for good; a device still
  // waiting to hear who drives keeps it open with its rows disabled.
  const block = useAgentChangeBlock(session)
  const changesBlocked = block !== null
  const underCard = block === "other" || block === "nobody"
  useEffect(() => {
    if (open && underCard) onClose()
  }, [open, underCard, onClose])
  const provider = defaultProviderForSession(spine, session)
  const cap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  const tabDisabled =
    changesBlocked || session.tabs.length >= cap || createTabInFlight.includes(session.id)
  const run = (act: () => void) => {
    onClose()
    act()
  }
  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : onClose())}>
      <DialogContent className="gap-3 sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add to this agent</DialogTitle>
          <DialogDescription>
            {block !== null
              ? addToAgentBlockHint(block)
              : "Another tab works in the same worktree; a terminal opens in its dock."}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-0.5">
          <NewRow
            icon={Bot}
            title="New agent tab"
            excerpt={
              <>
                Starts <InlineCode>{provider}</InlineCode> beside this agent&rsquo;s
                other tabs.
              </>
            }
            disabled={tabDisabled}
            autoFocus
            onActivate={() => run(() => addTab(session.id))}
          />
          <NewRow
            icon={SquareTerminal}
            title="New terminal"
            excerpt="Opens a shell in this agent's worktree, in its dock."
            disabled={changesBlocked}
            onActivate={() => run(() => createTerminal(session.id))}
          />
        </div>
      </DialogContent>
    </Dialog>
  )
}

function NewRow({
  icon: Icon,
  title,
  excerpt,
  disabled = false,
  autoFocus = false,
  onActivate,
}: {
  icon: LucideIcon
  title: string
  excerpt: ReactNode
  disabled?: boolean
  autoFocus?: boolean
  onActivate: () => void
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      autoFocus={autoFocus}
      onClick={onActivate}
      // The rows sit 2px apart with nothing else between them, and each is at
      // least 56px tall, well over the touch floor.
      className={cn(
        "flex min-h-14 w-full items-center gap-3 rounded-lg border border-transparent px-2.5 py-2 text-left transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50 motion-reduce:transition-none",
        FOCUS_RING,
      )}
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-muted text-foreground/90">
        <Icon className="size-[18px]" />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-sm font-medium">{title}</span>
        <span className="text-[12.5px] leading-snug text-muted-foreground">{excerpt}</span>
      </span>
    </button>
  )
}
