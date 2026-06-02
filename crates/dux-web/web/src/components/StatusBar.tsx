import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { reconnect, useDux } from "@/lib/store"
import type { SessionStatus } from "@/lib/types"

// Map a session status onto a Badge variant + label for the statusline.
const STATUS_BADGE: Record<
  SessionStatus,
  { variant: "default" | "secondary" | "outline"; label: string }
> = {
  active: { variant: "default", label: "active" },
  detached: { variant: "secondary", label: "detached" },
  exited: { variant: "outline", label: "exited" },
}

const CONN_LABEL: Record<string, string> = {
  open: "Connected",
  connecting: "Connecting",
  closed: "Offline",
  failed: "Connection failed",
}

export function StatusBar() {
  const { viewModel, selectedSessionId, lastMessage, conn } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const status = session ? STATUS_BADGE[session.status] : null

  return (
    <footer className="flex h-7 shrink-0 items-center justify-between border-t px-3 text-xs text-muted-foreground">
      <div className="flex min-w-0 items-center gap-2">
        {session ? (
          <>
            <span className="truncate font-mono">
              {session.provider} · {session.branch_name}
            </span>
            {status ? (
              <Badge variant={status.variant}>{status.label}</Badge>
            ) : null}
            {session.pr ? (
              <a
                href={session.pr.url}
                target="_blank"
                rel="noopener noreferrer"
                className="truncate text-foreground hover:underline"
              >
                PR #{session.pr.number} · {session.pr.title}
              </a>
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
