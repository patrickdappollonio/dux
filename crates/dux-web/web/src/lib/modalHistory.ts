// The dialogs, sheets and menus that are open right now, in the order they
// opened, so the browser's Back can close the top one.
//
// This module only keeps the list. It writes no history: the store is the one
// history writer, and it listens here and gives each open modal one entry of
// its own (see "Modal entries" in store.ts). Kept apart from the store so the
// shared UI primitives can register without importing the whole app state.

export interface OpenModal {
  id: number
  // Closes the modal the way its own Cancel or Escape does: the action it was
  // offering is abandoned, nothing is saved, confirmed or submitted.
  close: () => void
  // Set once the modal has lost its entry for good (Back closed it, or the page
  // moved under it). A spent modal that is still open is never given a new
  // one, so a modal that refused to close cannot turn Back into a dead press.
  spent: boolean
}

let modals: OpenModal[] = []
let nextId = 1
let listener: (() => void) | null = null

// Register an open modal. Returns the call that unregisters it, to be made
// whenever it closes, however it closes.
export function registerModal(close: () => void): () => void {
  const modal: OpenModal = { id: nextId++, close, spent: false }
  modals = [...modals, modal]
  listener?.()
  return () => {
    if (!modals.includes(modal)) return
    modals = modals.filter((m) => m !== modal)
    listener?.()
  }
}

// The modals open now, oldest first.
export function openModals(): readonly OpenModal[] {
  return modals
}

// Close these modals, newest first, marking each spent so it never gets an
// entry again.
export function closeModals(ids: readonly number[]): void {
  const closing = modals.filter((m) => ids.includes(m.id)).reverse()
  for (const modal of closing) {
    modal.spent = true
    modal.close()
  }
}

// Mark these modals spent without closing them: the page moved while they were
// open, so their entries went with it.
export function spendModals(ids: readonly number[]): void {
  for (const modal of modals) {
    if (ids.includes(modal.id)) modal.spent = true
  }
}

// The store's one subscription: told after every open and close.
export function onModalsChange(run: () => void): void {
  listener = run
}
