/// "1 unsaved change" or "N unsaved changes".
export function unsavedChangesLabel(count: number): string {
  return count === 1 ? "1 unsaved change" : `${count} unsaved changes`
}
