import { StatusBadge } from "@/components/StatusBadge"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { GitPullRequest } from "lucide-react"
import { reconnect, useDux } from "@/lib/store"
import type { PrView } from "@/lib/types"

// Colored PR badge matching GitHub/TUI semantics: green=open, purple=merged, red=closed.
function prBadgeClass(state: PrView["state"]): string {
  if (state === "open") return "border-transparent bg-green-600/15 text-green-500"
  if (state === "merged") return "border-transparent bg-purple-600/15 text-purple-400"
  return "border-transparent bg-red-600/15 text-red-400"
}

const CONN_LABEL: Record<string, string> = {
  open: "Connected",
  connecting: "Connecting",
  closed: "Offline",
  failed: "Connection failed",
}

export function StatusBar() {
  const { viewModel, selectedSessionId, selectedTarget, lastMessage, conn } =
    useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const focusLabel =
    selectedTarget?.kind === "terminal" ? "terminal" : "agent"

  return (
    <footer className="flex h-7 shrink-0 items-center justify-between border-t px-3 text-xs text-muted-foreground">
      <div className="flex min-w-0 items-center gap-2">
        {session ? (
          <>
            <Badge variant="outline">{focusLabel}</Badge>
            <span className="truncate font-mono">
              {session.provider} · {session.branch_name}
            </span>
            {session ? <StatusBadge status={session.status} /> : null}
            {session.pr ? (
              <Badge
                className={prBadgeClass(session.pr.state)}
                render={
                  <a
                    href={session.pr.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    title={session.pr.title}
                  >
                    <GitPullRequest data-icon="inline-start" />#{session.pr.number}
                  </a>
                }
              />
            ) : null}
          </>
        ) : (
          <span>No session</span>
        )}
      </div>
      <div className="flex min-w-0 items-center gap-2">
        {conn === "failed" ? (
          <Button variant="outline" size="sm" onClick={reconnect}>
            Reconnect
          </Button>
        ) : (
          <span>{CONN_LABEL[conn]}</span>
        )}
        <span className="truncate">{lastMessage}</span>
      </div>
    </footer>
  )
}
