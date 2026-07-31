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

// What a URL naming an agent this workspace does not have renders: a stale
// bookmark, a shared link to an agent someone deleted, or the browser's Back
// button landing on one. Saying so is the point: quietly showing the hub would
// leave the address bar naming an agent that is not on screen. Built from the
// shared empty-state primitives, the same ones the changed-files pane uses, so
// it reads as part of the app rather than an error page.
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
        {/* Through `navigateUp`, which REWRITES this particular entry rather
            than pushing home on top of it. Up pushes everywhere else, because
            everywhere else it moves between two real positions; leaving a bad
            address is a CORRECTION, and pushing would leave the dead end sitting
            one Back away. On a phone this screen replaces the whole shell, so
            this button is the only way out of it. */}
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
