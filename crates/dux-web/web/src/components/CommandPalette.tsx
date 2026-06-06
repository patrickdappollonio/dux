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
  logout,
  openCheckoutDefaultBranch,
  openCommit,
  openCreateAgent,
  openCreateAgentFromPr,
  openForkAgent,
  openRename,
  pullProject,
  reconnectSession,
  selectSession,
  setPaletteOpen,
  socket,
  useDux,
} from "@/lib/store"
import { PALETTE_HANDLERS } from "@/lib/paletteRegistry"

export function CommandPalette() {
  const { paletteOpen, viewModel, selectedSessionId, auth } = useDux()

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

  // Run a registry command by its core id. Entries whose id lacks a handler are
  // never rendered (filtered below), but guard here too so a stale ViewModel id
  // surfaces as a dev warning rather than a silent no-op.
  function runPaletteCommand(id: string) {
    const handler = PALETTE_HANDLERS[id]
    if (!handler) {
      console.warn(`No web handler registered for palette command "${id}"`)
      return
    }
    handler()
    close()
  }

  const sessions = viewModel?.sessions ?? []
  const projects = viewModel?.projects ?? []
  // The "Commands" group is driven by the surface-aware core registry: the
  // ViewModel projects the Web/Both subset (name + description, canonical
  // order); we render each entry whose id has a web handler.
  const paletteCommands = (viewModel?.palette_commands ?? []).filter(
    (cmd) => cmd.id in PALETTE_HANDLERS
  )
  // The "New agent from PR" entries are gated on GitHub/`gh` availability,
  // mirroring the TUI, which hides its `new-agent-from-pr` command in that state.
  const ghAvailable = viewModel?.gh_available ?? false

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
              {projects.map((p) => (
                <CommandItem
                  key={`checkout-default-${p.id}`}
                  className="cursor-pointer"
                  onSelect={() => {
                    // Route through the confirm dialog — the checkout moves HEAD
                    // in the shared source checkout, so it is gated like the menu.
                    openCheckoutDefaultBranch(p.id)
                    close()
                  }}
                >
                  Checkout default branch for {p.name}…
                </CommandItem>
              ))}
              {ghAvailable &&
                projects.map((p) => (
                  <CommandItem
                    key={`new-agent-from-pr-${p.id}`}
                    className="cursor-pointer"
                    onSelect={() => {
                      openCreateAgentFromPr(p.id)
                      close()
                    }}
                  >
                    New agent from PR in {p.name}…
                  </CommandItem>
                ))}
            </CommandGroup>
            <CommandSeparator />
          </>
        )}

        {/* Registry-driven global commands (the surface-aware core registry's
            Web/Both subset). Name + description mirror the TUI palette's
            presentation. Replaces the former hand-written Workspace and Sort
            groups so there is exactly one source of truth and no duplicates. */}
        {paletteCommands.length > 0 && (
          <>
            <CommandGroup heading="Commands">
              {paletteCommands.map((cmd) => (
                <CommandItem
                  key={cmd.id}
                  value={`${cmd.id} ${cmd.description}`}
                  className="cursor-pointer"
                  onSelect={() => runPaletteCommand(cmd.id)}
                >
                  <span className="font-medium">{cmd.id}</span>
                  <span className="text-muted-foreground">{cmd.description}</span>
                </CommandItem>
              ))}
            </CommandGroup>
            <CommandSeparator />
          </>
        )}

        {/* Web-only: not a TUI palette command (no BINDING_DEFS/core entry), so
            it stays hand-written. Overwrites config.toml from the running
            config — paired with the registry's reload-config. Log out is also
            web-only (the TUI has no session to end) and is shown only when auth
            is on AND a session is active — following the recover-config
            precedent of a hand-written entry that never touches the core
            registry. */}
        <CommandGroup heading="Config">
          <CommandItem
            className="cursor-pointer"
            onSelect={() => {
              socket.sendCommand("recover_config", {})
              close()
            }}
          >
            Recover config (overwrite config.toml)
          </CommandItem>
          {auth.phase === "authed" ? (
            <CommandItem
              className="cursor-pointer"
              onSelect={() => {
                void logout()
                close()
              }}
            >
              Log out{auth.username ? ` (${auth.username})` : ""}
            </CommandItem>
          ) : null}
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
