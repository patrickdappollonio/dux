import { useSyncExternalStore } from "react"

// WHICH MOUNTED PANE CAN ATTACH A FILE, keyed by PTY id.
//
// The row `⋯` menus (an agent row, a terminal row) are the desktop and
// keyboard-only path into the upload journey: there is no drag gesture to make
// and nothing on the clipboard. But a row is not a pane, and the upload has to
// travel through the PANE's own already-gated socket and land in the PANE's own
// sink (a compose draft or the terminal), never a side channel that would step
// around the input-ownership gate. So the pane publishes a capability while it
// is mounted, and the row menu borrows it.
//
// LIVE OWNERSHIP IS PART OF THE REGISTRATION, not something the menu checks
// afterwards. A viewer's pane mounts completely (it renders the take-over card
// over a live terminal), so a registration that ignored ownership would offer
// an attach whose every file stranded as saved-but-not-sent. The pane registers
// only while it owns the input and uploads are switched on, and retires the
// moment either stops being true.
//
// A dormant tab never mounts a pane, so nothing here can force-launch one: the
// item is simply absent, which is the row-menu convention for an inert action.

/// Open the picker and upload whatever is chosen into this pane's own sink.
type AttachFn = () => void

const capabilities = new Map<string, AttachFn>()
const listeners = new Set<() => void>()
// A monotonic counter IS the snapshot: `useSyncExternalStore` compares
// snapshots by value, and a Map's identity would either never change (mutation
// in place) or change on every read (a fresh copy), neither of which it can
// work with.
let version = 0

function publish(): void {
  version++
  for (const listener of listeners) listener()
}

/**
 * Publish this pane's attach capability. Returns the retirement.
 *
 * LAST WRITE WINS, and the retirement is symmetric: it removes the entry only
 * while the entry is still the one this call installed. The guard exists for
 * genuine replacement: when a second pane registers the same pty id before the
 * first unmounts (a cross-commit replacement), the first pane's late cleanup
 * must not retire the live pane's capability; an unconditional delete would,
 * and the menu item would vanish. React's StrictMode double-mount (setup,
 * cleanup, setup) retires and re-registers in order, so it is naturally
 * compatible with this guard.
 */
export function registerAttachCapability(
  ptyId: string,
  attach: AttachFn,
): () => void {
  capabilities.set(ptyId, attach)
  publish()
  return () => {
    if (capabilities.get(ptyId) !== attach) return
    capabilities.delete(ptyId)
    publish()
  }
}

/** The capability for one PTY id, or null when no mounted owner pane has it. */
export function attachCapabilityFor(ptyId: string): AttachFn | null {
  return capabilities.get(ptyId) ?? null
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => void listeners.delete(listener)
}

function snapshot(): number {
  return version
}

/**
 * The first attachable PTY among `ptyIds`, or null when none is.
 *
 * An agent passes its session-slot id plus every tab id, because any one of its
 * panes can be the mounted one; a terminal passes its single id. The array is
 * read during render rather than memoized: the subscription above is what
 * re-renders the menu when a pane mounts, takes over, or goes away.
 */
export function useAttachCapability(ptyIds: string[]): AttachFn | null {
  useSyncExternalStore(subscribe, snapshot, snapshot)
  for (const id of ptyIds) {
    const attach = capabilities.get(id)
    if (attach) return attach
  }
  return null
}

/** Test-only: forget every registration between cases. */
export function resetAttachCapabilities(): void {
  capabilities.clear()
  publish()
}
