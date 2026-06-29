import { useEffect } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  closeKillRunning,
  deleteTerminal,
  killSessionPty,
  useDux,
} from "@/lib/store"
import { terminalForeground } from "@/lib/terminals"

// The kill-running modal (Ctrl+K "kill-running"), the web counterpart to the
// TUI's kill-running modal. It lists every RUNNING runtime — active agents
// (those with a live PTY) and live companion terminals — and force-kills each on
// demand. Killing an agent only DETACHES it (the worktree and session survive,
// it can be reconnected); killing a terminal destroys it. The modal itself is
// the deliberate, clearly-labelled destructive surface, so each row kills
// immediately rather than opening a second confirm. The lists are derived live
// from the spine, so a runtime that exits (or is killed) drops from the list on
// the next `sessions.changed` refetch.
export function KillRunningDialog() {
  const { killRunningOpen, spine } = useDux()

  const sessions = spine?.sessions ?? []
  // Agents with a live PTY are exactly the "active" ones (the PTY-removed states
  // are "detached"/"exited"). Killing moves them to detached.
  const agents = sessions.filter((s) => s.status === "active")
  // Every companion-terminal row in the spine is a live PTY (terminals are never
  // persisted/detached — existence == running).
  const terminals = sessions.flatMap((s) =>
    s.terminals.map((terminal) => ({ session: s, terminal })),
  )
  const nothingRunning = agents.length === 0 && terminals.length === 0

  // If everything has been killed (or exited) while the modal is open, close it
  // so the user isn't left staring at an empty list.
  useEffect(() => {
    if (killRunningOpen && nothingRunning) {
      closeKillRunning()
    }
  }, [killRunningOpen, nothingRunning])

  function handleOpenChange(open: boolean) {
    if (!open) closeKillRunning()
  }

  return (
    <Dialog open={killRunningOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Kill running processes</DialogTitle>
          <DialogDescription>
            Force-kill a hung agent or terminal. Agents detach and can be
            reconnected; terminals are destroyed.
          </DialogDescription>
        </DialogHeader>

        <div className="flex max-h-72 flex-col gap-1 overflow-y-auto">
          {agents.map((s) => (
            <div
              key={s.id}
              className="flex items-center justify-between gap-3 rounded-md px-2 py-1.5 hover:bg-muted"
            >
              <div className="min-w-0">
                <div className="truncate text-sm">
                  {s.title ?? s.branch_name}
                </div>
                <div className="truncate font-mono text-xs text-muted-foreground">
                  agent · {s.provider}
                </div>
              </div>
              <Button
                size="sm"
                variant="destructive"
                onClick={() => killSessionPty(s.id)}
              >
                Kill
              </Button>
            </div>
          ))}

          {terminals.map(({ session, terminal }) => {
            const foreground = terminalForeground(terminal)
            return (
              <div
                key={terminal.id}
                className="flex items-center justify-between gap-3 rounded-md px-2 py-1.5 hover:bg-muted"
              >
                <div className="min-w-0">
                  <div className="truncate text-sm">{terminal.label}</div>
                  <div className="truncate font-mono text-xs text-muted-foreground">
                    terminal{foreground ? ` · ${foreground}` : ""} ·{" "}
                    {session.title ?? session.branch_name}
                  </div>
                </div>
                <Button
                  size="sm"
                  variant="destructive"
                  onClick={() => deleteTerminal(terminal.id)}
                >
                  Kill
                </Button>
              </div>
            )
          })}

          {nothingRunning ? (
            <p className="px-2 py-6 text-center text-sm text-muted-foreground">
              Nothing is running.
            </p>
          ) : null}
        </div>

        {/* Misclick-safe spacing between the list and the footer button. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeKillRunning}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
