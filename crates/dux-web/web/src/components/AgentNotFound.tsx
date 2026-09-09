import { SearchX } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { navigateUp } from "@/lib/store"

// What a URL naming an agent this workspace does not have renders. Saying so is
// the point: quietly showing the hub would leave the address bar naming an agent
// that is not on screen. Built from the shared empty-state primitives, so it
// reads as part of the app rather than an error page.
export function AgentNotFound({ sessionId }: { sessionId: string }) {
  return (
    <Empty className="h-full border-0">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <SearchX />
        </EmptyMedia>
        <EmptyTitle>Agent not found</EmptyTitle>
        <EmptyDescription>
          This link points at an agent that is no longer in this workspace,
          probably because it was deleted. Its id was{" "}
          <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs break-all">
            {sessionId}
          </code>
          .
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        {/* Through `navigateUp`, which rewrites this entry rather than pushing
          * home on top of it: leaving a bad address is a correction, and pushing
          * would leave the dead end one Back away. Up pushes everywhere else,
          * where it moves between two real positions. */}
        <Button
          variant="outline"
          className="max-md:min-h-10"
          onClick={() => navigateUp()}
        >
          Back to agents
        </Button>
      </EmptyContent>
    </Empty>
  )
}
