import { SearchX, SquareTerminal } from "lucide-react"
import { useId } from "react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { Button } from "@/components/ui/button"
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { InlineCode } from "@/components/ui/inline-code"
import {
  addToAgentBlockHint,
  useAgentChangeBlock,
} from "@/hooks/use-agent-active-elsewhere"
import { assertNever } from "@/lib/assertNever"
import {
  createProjectTerminal,
  createStandaloneTerminal,
  createTerminal,
  navigateUp,
  useDux,
  type RouteNotFound,
} from "@/lib/store"
import { matchOwner, type TerminalOwnerRef } from "@/lib/terminalOwner"

// What every surface renders for an address it cannot resolve.
export function RouteNotFoundScreen({ missing }: { missing: RouteNotFound }) {
  switch (missing.kind) {
    case "agent":
      return <AgentNotFound sessionId={missing.sessionId} />
    case "terminal":
      return <TerminalNotFound terminalId={missing.terminalId} owner={missing.owner} />
    default:
      return assertNever(missing)
  }
}

// A link to a terminal that has closed. Terminals end all the time, so the
// screen offers the obvious next step: a new terminal where that one was.
export function TerminalNotFound({
  terminalId,
  owner,
}: {
  terminalId: string
  owner: TerminalOwnerRef
}) {
  const { spine } = useDux()
  const newTerminal = ownerExists(owner, spine) ? newTerminalFor(owner) : null
  // A terminal of an agent's joins the agent's row, so it waits while another
  // device drives that row, as every New terminal on the agent's pane does.
  const agent =
    owner.kind === "session"
      ? spine?.sessions.find((s) => s.id === owner.sessionId)
      : undefined
  const block = useAgentChangeBlock(agent)
  const blocked = block !== null
  const reasonId = useId()
  return (
    <Empty className="h-full border-0">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <SearchX />
        </EmptyMedia>
        <EmptyTitle>Terminal not found</EmptyTitle>
        <EmptyDescription>
          This link points at a terminal that is no longer running, probably
          because it was closed or its shell exited. Its id was{" "}
          <InlineCode>{terminalId}</InlineCode>
          .
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent className="flex-row flex-wrap justify-center gap-4">
        {/* Filled: opening a terminal is this screen's primary act, and the
          * way up beside it stays outline, as on the agent's screen. A new
          * terminal is a position of its own, so this one stays one Back
          * behind it, where the user found it. */}
        {newTerminal !== null ? (
          <Button
            className="max-md:min-h-10 pointer-coarse:min-h-10"
            disabled={blocked}
            aria-describedby={blocked ? reasonId : undefined}
            onClick={newTerminal}
          >
            <SquareTerminal />
            New terminal
          </Button>
        ) : null}
        <Button
          variant="outline"
          className="max-md:min-h-10 pointer-coarse:min-h-10"
          onClick={() => navigateUp()}
        >
          Back to agents
        </Button>
        {/* Said in words, since a disabled button answers no hover and a
          * finger has none. */}
        {newTerminal !== null && block !== null ? (
          <p id={reasonId} className="w-full text-center text-sm text-muted-foreground">
            {addToAgentBlockHint(block)}
          </p>
        ) : null}
      </EmptyContent>
    </Empty>
  )
}

// Whether the owner the address named is still there to open a terminal for.
function ownerExists(
  owner: TerminalOwnerRef,
  spine: { sessions: { id: string }[]; projects: { id: string }[] } | null,
): boolean {
  return matchOwner(owner, {
    session: ({ sessionId }) => spine?.sessions.some((s) => s.id === sessionId) === true,
    project: ({ projectId }) => spine?.projects.some((p) => p.id === projectId) === true,
    standalone: () => true,
  })
}

// The action that opens a terminal for the owner the address named.
function newTerminalFor(owner: TerminalOwnerRef): () => void {
  return matchOwner<() => void>(owner, {
    session: ({ sessionId }) => () => createTerminal(sessionId),
    project: ({ projectId }) => () => createProjectTerminal(projectId),
    standalone: () => () => createStandaloneTerminal(),
  })
}
