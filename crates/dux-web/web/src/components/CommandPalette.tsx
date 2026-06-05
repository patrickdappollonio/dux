import { useEffect } from "react"
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command"
import {
  openAddProject,
  openCommit,
  openCreateAgent,
  openForkAgent,
  openGlobalEnv,
  openRename,
  pullProject,
  reconnectSession,
  selectSession,
  setPaletteOpen,
  socket,
  useDux,
} from "@/lib/store"

export function CommandPalette() {
  const { paletteOpen, viewModel, selectedSessionId } = useDux()

  const selectedSession = viewModel?.sessions.find(
    (s) => s.id === selectedSessionId
  ) ?? null

  // Global ⌘K / Ctrl-K handler.
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault()
        setPaletteOpen(!paletteOpen)
      }
    }
    window.addEventListener("keydown", handleKeyDown)
    return () => window.removeEventListener("keydown", handleKeyDown)
  }, [paletteOpen])

  function close() {
    setPaletteOpen(false)
  }

  function handleToggleAutoReopen() {
    if (!selectedSession) return
    socket.sendCommand("toggle_agent_auto_reopen", {
      session_id: selectedSession.id,
      enabled: !selectedSession.auto_reopen_enabled,
    })
    close()
  }

  function handleCommit() {
    if (!selectedSessionId) return
    openCommit(selectedSessionId)
    close()
  }

  function handlePush() {
    if (!selectedSessionId) return
    socket.sendCommand("push", { session_id: selectedSessionId })
    close()
  }

  function handleSelectSession(id: string) {
    selectSession(id)
    close()
  }

  const sessions = viewModel?.sessions ?? []
  const projects = viewModel?.projects ?? []

  return (
    <CommandDialog open={paletteOpen} onOpenChange={setPaletteOpen}>
      <CommandInput placeholder="Type a command or search sessions…" />
      <CommandList>
        <CommandEmpty>No results found.</CommandEmpty>

        {selectedSession && (
          <>
            <CommandGroup heading="Session actions">
              <CommandItem
                className="cursor-pointer"
                onSelect={handleToggleAutoReopen}
              >
                Toggle auto-reopen (currently{" "}
                {selectedSession.auto_reopen_enabled ? "on" : "off"})
              </CommandItem>
              <CommandItem className="cursor-pointer" onSelect={handleCommit}>
                Commit…
              </CommandItem>
              <CommandItem className="cursor-pointer" onSelect={handlePush}>
                Push
              </CommandItem>
              <CommandItem
                className="cursor-pointer"
                onSelect={() => {
                  reconnectSession(selectedSession.id, false)
                  close()
                }}
              >
                Reconnect
              </CommandItem>
              <CommandItem
                className="cursor-pointer"
                onSelect={() => {
                  reconnectSession(selectedSession.id, true)
                  close()
                }}
              >
                Force reconnect (fresh)
              </CommandItem>
              <CommandItem
                className="cursor-pointer"
                onSelect={() => {
                  openRename(selectedSession.id)
                  close()
                }}
              >
                Rename…
              </CommandItem>
              <CommandItem
                className="cursor-pointer"
                onSelect={() => {
                  openForkAgent(selectedSession.id)
                  close()
                }}
              >
                Fork agent…
              </CommandItem>
              <CommandItem
                className="cursor-pointer"
                onSelect={() => {
                  openCreateAgent(selectedSession.project_id)
                  close()
                }}
              >
                New agent in this project…
              </CommandItem>
            </CommandGroup>
            <CommandSeparator />
          </>
        )}

        {/* Project-scoped actions live OUTSIDE the session group so agent-less
            projects stay reachable from the palette (the TUI's project refresh
            works off the selected project, no session required). */}
        {projects.length > 0 && (
          <>
            <CommandGroup heading="Projects">
              {projects.map((p) => (
                <CommandItem
                  key={p.id}
                  className="cursor-pointer"
                  onSelect={() => {
                    pullProject(p.id)
                    close()
                  }}
                >
                  Pull {p.name}…
                </CommandItem>
              ))}
            </CommandGroup>
            <CommandSeparator />
          </>
        )}

        <CommandGroup heading="Workspace">
          <CommandItem
            className="cursor-pointer"
            onSelect={() => {
              openAddProject()
              close()
            }}
          >
            Add project…
          </CommandItem>
          <CommandItem
            className="cursor-pointer"
            onSelect={() => {
              openGlobalEnv()
              close()
            }}
          >
            Global environment…
          </CommandItem>
          <CommandItem
            className="cursor-pointer"
            onSelect={() => {
              socket.sendCommand("reload_config", {})
              close()
            }}
          >
            Reload config from disk
          </CommandItem>
          <CommandItem
            className="cursor-pointer"
            onSelect={() => {
              socket.sendCommand("recover_config", {})
              close()
            }}
          >
            Recover config (overwrite config.toml)
          </CommandItem>
        </CommandGroup>
        <CommandSeparator />

        <CommandGroup heading="Switch session">
          {sessions.map((s) => (
            <CommandItem
              key={s.id}
              value={`${s.provider} ${s.branch_name} ${s.id}`}
              className="cursor-pointer"
              onSelect={() => handleSelectSession(s.id)}
            >
              {s.provider} · {s.branch_name}
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
