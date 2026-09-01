import { useSyncExternalStore } from "react"

import type { InputMenuGates } from "./inputMenu"

// WHERE THE INPUT `⋯` GOES WHEN THE PANE HAS NOWHERE TO PUT IT, keyed by PTY id.
//
// The input menu normally rides the bottom-most input row the pane renders: the
// compose row, the accessory bar's first row, or a minimal row of its own.
// Theater mode on a computer takes the whole window, and a bordered row under
// the terminal there is both a second `⋯` (the floating pill already carries
// one) and exactly the chrome the mode exists to remove.
//
// So in that state the pane renders no row and publishes its items here
// instead, and the pill folds them into its own single menu. One trigger on
// screen, and the typing-surface switch stays reachable in a mode that has
// taken every other anchor away.
//
// The pane publishes only while it has NO anchor of its own, so the items can
// never appear in two menus at once: a phone in theater still gets the row (its
// pill can end up under the soft keyboard) and publishes nothing.
//
// Keyed rather than a single slot, on the `attachRegistry` precedent: several
// panes can be mounted at once, and the pill must read the one it is painted
// over rather than whichever mounted last.

/// What one pane's input `⋯` would show if it had somewhere to render.
export type PaneInputMenu = {
  gates: InputMenuGates
  /// Which typing surface is live, for the switch item's wording.
  composeSurface: boolean
}

const menus = new Map<string, PaneInputMenu>()
const listeners = new Set<() => void>()
// A monotonic counter IS the snapshot, for the same reason `attachRegistry`
// uses one: `useSyncExternalStore` compares snapshots by value.
let version = 0

function publish(): void {
  version++
  for (const listener of listeners) listener()
}

/**
 * Publish this pane's input-menu content. Returns the retirement.
 *
 * LAST WRITE WINS, and the retirement removes the entry only while it is still
 * the one this call installed, so a replacement pane's registration survives
 * the outgoing pane's late cleanup. Same guard, and same reason, as
 * `registerAttachCapability`.
 */
export function registerPaneInputMenu(
  ptyId: string,
  menu: PaneInputMenu,
): () => void {
  menus.set(ptyId, menu)
  publish()
  return () => {
    if (menus.get(ptyId) !== menu) return
    menus.delete(ptyId)
    publish()
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => void listeners.delete(listener)
}

function snapshot(): number {
  return version
}

/** The published menu for one PTY id, or null when its pane has an anchor. */
export function usePaneInputMenu(ptyId: string): PaneInputMenu | null {
  useSyncExternalStore(subscribe, snapshot, snapshot)
  return menus.get(ptyId) ?? null
}

/** Test-only: forget every registration between cases. */
export function resetPaneInputMenus(): void {
  menus.clear()
  publish()
}
