import { SearchX } from "lucide-react"

import { Button } from "@/components/ui/button"
import { EmptyState } from "@/components/ui/empty"
import { InlineCode } from "@/components/ui/inline-code"
import { navigateUp } from "@/lib/store"

// What a URL naming an agent this workspace does not have renders. Saying so is
// the point: quietly showing the hub would leave the address bar naming an agent
// that is not on screen. Built as the shared empty state, so it
// reads as part of the app rather than an error page.
export function AgentNotFound({ sessionId }: { sessionId: string }) {
  return (
    <EmptyState
      size="page"
      icon={<SearchX />}
      title="Agent not found"
      description={
        <>
          This link points at an agent that is no longer in this workspace,
          probably because it was deleted. Its id was{" "}
          <InlineCode>{sessionId}</InlineCode>
          .
        </>
      }
      // Through `navigateUp`, which rewrites this entry rather than pushing
      // home on top of it: leaving a bad address is a correction, and pushing
      // would leave the dead end one Back away. Up pushes everywhere else,
      // where it moves between two real positions.
      actions={
        <Button
          variant="outline"
          className="max-md:min-h-10 pointer-coarse:min-h-10"
          onClick={() => navigateUp()}
        >
          Back to agents
        </Button>
      }
    />
  )
}
