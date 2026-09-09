import { useSyncExternalStore } from "react"

// Which mounted pane can attach a file, keyed by PTY id. A row menu is not a pane,
// and an upload must travel the pane's own gated socket into the pane's own sink,
// so the pane publishes this capability and the row menu borrows it.
//
// Live ownership is part of the registration rather than something the menu checks
// after the fact: a viewer's pane mounts completely, so a registration ignoring
// ownership would offer an attach whose every file stranded as saved-but-not-sent.
// A dormant tab mounts no pane, so nothing here can force-launch one.

/// Open the picker and upload whatever is chosen into this pane's own sink.
type AttachFn = () => void

const capabilities = new Map<string, AttachFn>()
const listeners = new Set<() => void>()
// A monotonic counter is the snapshot: `useSyncExternalStore` compares snapshots by
// value, and a Map's identity either never changes or changes on every read.
let version = 0

function publish(): void {
  version++
  for (const listener of listeners) listener()
}

/**
 * Publish this pane's attach capability. Returns the retirement.
 *
 * Last write wins, and the retirement removes the entry only while it is still the
 * one this call installed: when a second pane registers the same pty id before the
 * first unmounts, the first pane's late cleanup must not retire the live one.
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
 * The first attachable PTY among `ptyIds`, or null when none is. An agent passes its
 * session-slot id plus every tab id, since any of its panes may be the mounted one.
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
