// The user's explicit open/closed choice for the sidebar's Inactive tail,
// page-load scoped. `null` means the automation decides (open while the whole
// workspace is dormant, collapsed once any agent is active; the TUI's rule).
//
// Module state rather than component state on purpose: QuietTail unmounts on
// ordinary navigation (the mobile hub round trip, the nothing-matches search
// branch), and a remount must not silently discard an explicit collapse. The
// TUI's twin flag (`inactive_collapse_overridden`) lives for the whole app
// run; a page load is the web's equivalent lifetime.
let choice: boolean | null = null

export function quietTailManualChoice(): boolean | null {
  return choice
}

export function setQuietTailManualChoice(next: boolean): void {
  choice = next
}

export function resetQuietTailManualChoiceForTests(): void {
  choice = null
}
