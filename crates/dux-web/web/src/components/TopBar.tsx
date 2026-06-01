import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { setPaletteOpen, useDux } from "@/lib/store"

function ConnIndicator() {
  const { conn } = useDux()
  const map = {
    open: { label: "Connected", variant: "default" },
    connecting: { label: "Connecting", variant: "secondary" },
    closed: { label: "Disconnected", variant: "outline" },
  } as const
  const { label, variant } = map[conn]
  return <Badge variant={variant}>{label}</Badge>
}

export function TopBar() {
  const { viewModel, selectedSessionId } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)

  return (
    <header className="flex h-10 shrink-0 items-center gap-3 border-b border-border bg-background px-3">
      <div className="flex items-center gap-2 text-sm">
        <span className="font-semibold tracking-tight">dux</span>
        {session && (
          <span className="font-mono text-xs text-muted-foreground">
            {session.branch_name}
          </span>
        )}
      </div>

      <div className="flex flex-1 justify-center">
        <Button
          variant="outline"
          size="sm"
          onClick={() => setPaletteOpen(true)}
          className="gap-2 text-muted-foreground"
        >
          <span className="font-mono text-xs">⌘K</span>
          <span>Search…</span>
        </Button>
      </div>

      <ConnIndicator />
    </header>
  )
}
