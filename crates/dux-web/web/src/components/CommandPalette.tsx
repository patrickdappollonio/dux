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
  selectSession,
  setPaletteOpen,
  socket,
  useDux,
} from "@/lib/store"
import { groupPaletteCommands } from "@/lib/paletteGroups"
import { PALETTE_HANDLERS } from "@/lib/paletteRegistry"

export function CommandPalette() {
  const { paletteOpen, viewModel, auth } = useDux()

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

  // Run a registry command by its core id. Entries whose id lacks a handler are
  // never rendered (filtered below), but guard here too so a stale ViewModel id
  // surfaces as a dev warning rather than a silent no-op. Every handler is
  // GLOBAL (acts on no selected target) — the palette is strictly app-menu
  // shaped; target actions live in the rows' triple-dot menus.
  function runPaletteCommand(id: string) {
    const handler = PALETTE_HANDLERS[id]
    if (!handler) {
      console.warn(`No web handler registered for palette command "${id}"`)
      return
    }
    handler()
    close()
  }

  function handleSelectSession(id: string) {
    selectSession(id)
    close()
  }

  const sessions = viewModel?.sessions ?? []
  // The "Commands" groups are driven by the surface-aware core registry: the
  // ViewModel projects the Web/Both subset (id + description, canonical order);
  // we render each entry whose id has a web handler, bucketed into app-menu
  // groups (Configuration / View / Projects) for the menu-like feel.
  const paletteCommands = (viewModel?.palette_commands ?? []).filter(
    (cmd) => cmd.id in PALETTE_HANDLERS
  )
  const commandGroups = groupPaletteCommands(paletteCommands)

  return (
    <CommandDialog
      open={paletteOpen}
      onOpenChange={setPaletteOpen}
      className="sm:max-w-2xl"
    >
      <CommandInput placeholder="Type a command or search sessions…" />
      <CommandList>
        <CommandEmpty>No results found.</CommandEmpty>

        {/* Registry-driven global commands (the surface-aware core registry's
            Web/Both subset), grouped app-menu-style. Name + description mirror
            the TUI palette's presentation. Every entry is global — no
            target-specific actions live in the palette (those are in the rows'
            triple-dot menus). */}
        {commandGroups.map(({ group, commands }) => (
          <CommandGroup key={group} heading={group}>
            {commands.map((cmd) => (
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
        ))}
        {commandGroups.length > 0 ? <CommandSeparator /> : null}

        {/* Web-only: not a TUI palette command (no BINDING_DEFS/core entry), so
            it stays hand-written. Both are GLOBAL. Recover config overwrites
            config.toml from the running config — paired with the registry's
            reload-config. Log out is web-only (the TUI has no session to end)
            and shown only when auth is on — following the recover-config
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

        {/* Switch session is global navigation (the menu analog of "Window >
            switch to…"), not an action ON a target — it selects which session
            the center pane streams. The per-session ACTIONS live in the session
            row's triple-dot menu. */}
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
