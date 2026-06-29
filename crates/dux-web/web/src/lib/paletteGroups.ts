// App-menu-style grouping for the global command palette.
//
// The Rust `dux_core::palette` registry is the single source of truth for which
// commands exist and which surface each appears on; `paletteRegistry` supplies
// the per-id handler. Grouping, by contrast, is a pure WEB-presentation concern
// (the TUI palette lists flat in keybinding order), so it lives here rather than
// bloating every core registry row with a web-only label. Each web command id
// maps to a group heading; `groupPaletteCommands` buckets the bootstrap
// document's `palette_commands` into those groups in registry order. The handler-coverage
// test pins that every web command id has a group, so a new Web/Both command
// can't ship ungrouped.

import type { PaletteCommandView } from "@/lib/types"

// The app-menu groups, in display order. Commands appear under their group in
// the order the registry (and thus `palette_commands`) yields them.
export const PALETTE_GROUP_ORDER = [
  "Configuration",
  "View",
  "Runtime",
  "Projects",
] as const

export type PaletteGroup = (typeof PALETTE_GROUP_ORDER)[number]

// id (dashed core command name) -> app-menu group. Every web-surfaced registry
// id must appear here; the pin test fails otherwise so grouping can't silently
// drift from the registry.
const GROUP_BY_ID: Record<string, PaletteGroup> = {
  "add-project": "Projects",
  "configure-global-env": "Configuration",
  "edit-macros": "Configuration",
  "reload-config": "Configuration",
  "toggle-github-integration": "Configuration",
  "toggle-randomized-pet-name-default": "Configuration",
  "sort-agents-by-created": "View",
  "sort-agents-by-name": "View",
  "sort-agents-by-updated": "View",
  "toggle-pr-banner-position": "View",
  "toggle-remove-git-pane": "View",
  "kill-running": "Runtime",
}

// The ids that carry a group mapping — exported for the coverage test's REVERSE
// check (every mapped id must still be a live web command, so a removed command
// left behind here is caught rather than silently lingering).
export const GROUPED_PALETTE_IDS = Object.keys(GROUP_BY_ID)

// The group for a web command id, or null when none is mapped (a registry
// addition without a group — caught by the coverage test). CommandPalette skips
// ungrouped ids so an unmapped command never crashes the render.
export function paletteGroupFor(id: string): PaletteGroup | null {
  return GROUP_BY_ID[id] ?? null
}

// Bucket the bootstrap document's `palette_commands` into app-menu groups, preserving the
// registry's canonical order within each group and dropping the empty groups.
// `id in PALETTE_HANDLERS` filtering still happens in CommandPalette; this only
// arranges the entries that survive that filter.
export function groupPaletteCommands(
  commands: PaletteCommandView[],
): { group: PaletteGroup; commands: PaletteCommandView[] }[] {
  return PALETTE_GROUP_ORDER.map((group) => ({
    group,
    commands: commands.filter((cmd) => paletteGroupFor(cmd.id) === group),
  })).filter((bucket) => bucket.commands.length > 0)
}
