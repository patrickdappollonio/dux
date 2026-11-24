import { Bot, Plus, SquareTerminal } from "lucide-react"
import type { LucideIcon } from "lucide-react"
import { useState, type ReactNode } from "react"

import { IconTile } from "@/components/IconTile"
import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { Modal, useHeldWhileClosed } from "@/components/modal"
import { HeaderButton } from "@/components/HeaderButton"
import { useFoldHold } from "@/hooks/use-fold-to-fit"
import { InlineCode } from "@/components/ui/inline-code"
import {
  addToAgentBlockHint,
  useAgentChangeBlock,
  useRowDriverLabel,
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
  const device = useRowDriverLabel({ sessionId: session.id })
  // Mounted whatever the button shows, so the dialog hears the block arrive
  // while it is open and closes for good.
  const dialog = (
    <StackNewDialog session={session} open={adding} onClose={() => setAdding(false)} />
  )
  return (
    <>
      <ExplainedWhenDisabled
        disabled={block !== null}
        hint={block === null ? "New agent tab or terminal" : addToAgentBlockHint(block, device)}
        className="rounded-lg"
      >
        <HeaderButton
          icon={<Plus />}
          label="New tab…"
          disabled={block !== null}
          onClick={() => setAdding(true)}
        />
      </ExplainedWhenDisabled>
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
  // The header's controls stay as they are while this is open and until its
  // close has finished, so it is never taken from under a person.
  const settleFold = useFoldHold(open)
  // The take-over card coming up closes the dialog for good; a device still
  // waiting to hear who drives keeps it open with its rows disabled.
  const block = useAgentChangeBlock(session)
  const device = useRowDriverLabel({ sessionId: session.id })
  const changesBlocked = block !== null
  const underCard = block === "other" || block === "nobody"
  const provider = defaultProviderForSession(spine, session)
  const cap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  // Held while the close animates: the tab a press adds is in flight by then.
  const tabDisabled = useHeldWhileClosed(
    open,
    changesBlocked || session.tabs.length >= cap || createTabInFlight.includes(session.id),
  )
  const run = (act: () => void) => {
    onClose()
    act()
  }
  return (
    <Modal
      kind="picker"
      open={open}
      vanished={open && underCard}
      onClose={() => onClose()}
      onClosed={settleFold}
      size="md"
      title="Add to this agent"
      description={
        block !== null
          ? addToAgentBlockHint(block, device)
          : "Another tab works in the same worktree; a terminal opens in its dock."
      }
    >
      <div className="flex flex-col gap-0.5">
        <NewRow
          icon={Bot}
          title="New agent tab"
          excerpt={
            <>
              Starts <InlineCode>{provider}</InlineCode> beside this agent's other tabs.
            </>
          }
          disabled={tabDisabled}
          onActivate={() => run(() => addTab(session.id))}
        />
        <NewRow
          icon={SquareTerminal}
          title="New terminal tab"
          excerpt="Opens a shell in this agent's worktree, in its dock."
          disabled={changesBlocked}
          onActivate={() => run(() => createTerminal(session.id))}
        />
      </div>
    </Modal>
  )
}

function NewRow({
  icon: Icon,
  title,
  excerpt,
  disabled = false,
  onActivate,
}: {
  icon: LucideIcon
  title: string
  excerpt: ReactNode
  disabled?: boolean
  onActivate: () => void
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onActivate}
      // The rows sit 2px apart with nothing else between them, and each is at
      // least 56px tall, well over the touch floor.
      className={cn(
        "flex min-h-14 w-full items-center gap-3 rounded-lg border border-transparent px-2.5 py-2 text-left transition-colors hover:bg-accent focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50 motion-reduce:transition-none",
        FOCUS_RING,
      )}
    >
      <IconTile size="md" use="action">
        <Icon />
      </IconTile>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-sm font-medium">{title}</span>
        <span className="text-[12.5px] leading-snug text-muted-foreground">{excerpt}</span>
      </span>
    </button>
  )
}
