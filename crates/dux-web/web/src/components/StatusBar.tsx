import { useDux } from "@/lib/store"

export function StatusBar() {
  const { viewModel, selectedSessionId, lastMessage } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)

  return (
    <footer className="flex h-6 shrink-0 items-center justify-between gap-3 border-t border-border bg-background px-3 text-xs text-muted-foreground">
      <span className="truncate font-mono">{session?.branch_name ?? ""}</span>
      <span className="truncate">{lastMessage}</span>
    </footer>
  )
}
