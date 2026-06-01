import { ScrollArea } from "@/components/ui/scroll-area"
import { selectSession, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionStatus, SessionView } from "@/lib/types"

const STATUS_DOT: Record<SessionStatus, string> = {
  active: "bg-emerald-500",
  detached: "bg-amber-500",
  exited: "bg-zinc-500",
}

function SessionRow({
  session,
  selected,
}: {
  session: SessionView
  selected: boolean
}) {
  return (
    <button
      type="button"
      onClick={() => selectSession(session.id)}
      className={cn(
        "flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors",
        selected
          ? "bg-accent text-accent-foreground"
          : "text-muted-foreground hover:bg-muted hover:text-foreground"
      )}
    >
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          STATUS_DOT[session.status]
        )}
      />
      <span className="truncate">
        {session.provider} · {session.branch_name}
      </span>
    </button>
  )
}

export function Sidebar() {
  const { viewModel, selectedSessionId } = useDux()
  const sessions = viewModel?.sessions ?? []
  const projects = viewModel?.projects ?? []

  // Group sessions by their owning project, preserving first-seen order.
  const order: string[] = []
  const grouped = new Map<string, SessionView[]>()
  for (const session of sessions) {
    let bucket = grouped.get(session.project_id)
    if (!bucket) {
      bucket = []
      grouped.set(session.project_id, bucket)
      order.push(session.project_id)
    }
    bucket.push(session)
  }

  const projectName = (id: string) =>
    projects.find((p) => p.id === id)?.name ?? id.slice(0, 8)

  return (
    <ScrollArea className="h-full bg-sidebar">
      <div className="flex flex-col gap-3 p-2">
        {order.length === 0 && (
          <p className="px-2 py-1 text-xs text-muted-foreground">
            No sessions yet.
          </p>
        )}
        {order.map((projectId) => (
          <div key={projectId} className="flex flex-col gap-0.5">
            <h2 className="truncate px-2 py-1 text-[0.7rem] font-semibold tracking-wide text-muted-foreground uppercase">
              {projectName(projectId)}
            </h2>
            {grouped.get(projectId)!.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                selected={session.id === selectedSessionId}
              />
            ))}
          </div>
        ))}
      </div>
    </ScrollArea>
  )
}
