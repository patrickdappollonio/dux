import { useEffect } from "react"
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import { setPaletteOpen, useDux } from "@/lib/store"
import { groupPaletteCommands } from "@/lib/paletteGroups"
import { PALETTE_HANDLERS } from "@/lib/paletteRegistry"

// The core command id whose visibility is gated on GitHub integration. Named so
// a rename in the Rust registry surfaces as a failing reference here rather than
// a silently-ineffective string literal.
const PR_BANNER_POSITION_COMMAND = "toggle-pr-banner-position"

export function CommandPalette() {
  const { paletteOpen, bootstrap } = useDux()

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

  // The "Commands" groups are driven by the surface-aware core registry: the
  // ViewModel projects the Web/Both subset (id + description, canonical order);
  // we render each entry whose id has a web handler, bucketed into app-menu
  // groups (Configuration / View / Projects) for the menu-like feel.
  // Hide the PR-banner-position toggle when GitHub integration is OFF: there is
  // no banner to position. Gate on the raw `github_integration` flag, NOT
  // `gh_available` (the composite) — the user's banner preference is still
  // meaningful when integration is on but `gh` is momentarily unreachable.
  const githubIntegration = bootstrap?.github_integration ?? false
  const paletteCommands = (bootstrap?.palette_commands ?? []).filter(
    (cmd) =>
      cmd.id in PALETTE_HANDLERS &&
      (cmd.id !== PR_BANNER_POSITION_COMMAND || githubIntegration)
  )
  const commandGroups = groupPaletteCommands(paletteCommands)

  return (
    <CommandDialog
      open={paletteOpen}
      onOpenChange={setPaletteOpen}
      className="sm:max-w-2xl"
    >
      <CommandInput placeholder="Type a command…" />
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
                // Three aligned columns (tabwriter-style): a fixed-width command
                // name so every description starts at the same x across rows,
                // then a trailing `auto` column. That last column is load-bearing:
                // `CommandItem` always appends an (invisible, opacity-0) checked-
                // state CheckIcon after its children, so a 2-column grid orphaned
                // it onto an implicit SECOND grid row — doubling every row's
                // height with nothing visible. Giving it a home column keeps rows
                // single-line. `items-center` (not baseline) vertically centers
                // the monospace id against the description.
                className="grid cursor-pointer grid-cols-[12rem_minmax(0,1fr)_auto] items-center gap-3"
                onSelect={() => runPaletteCommand(cmd.id)}
              >
                <span className="truncate font-mono text-xs">{cmd.id}</span>
                <span className="truncate text-sm text-muted-foreground">
                  {cmd.description}
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        ))}
      </CommandList>
    </CommandDialog>
  )
}
