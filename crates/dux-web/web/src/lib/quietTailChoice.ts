// The user's explicit open/closed choice for the sidebar's Inactive tail, scoped to
// the page load. `null` hands the decision to the automation: open while the whole
// workspace is dormant, collapsed once any agent is active.
//
// Module state rather than component state because QuietTail unmounts on ordinary
// navigation, and a remount must not discard an explicit collapse.
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
