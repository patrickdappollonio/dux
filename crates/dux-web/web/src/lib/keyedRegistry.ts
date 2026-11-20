import { useSyncExternalStore } from "react"

// One mounted pane, one key, one answer: the shape every "the pane knows, the
// chrome outside it has to ask" registry in this app has. A pane publishes under
// its pty id and a menu, a header or a shell reads that id, because several
// panes can be mounted at once and a reader must get its own rather than
// whichever published last.
//
// Extracted because the three that exist were the same forty lines three times,
// and a fix to the retirement rule in one of them would have been a fix in one
// of them.

/// What one registry offers. `T` is stored by identity, never compared by
/// value, which is why a caller publishing a primitive has to box it.
export interface KeyedRegistry<T> {
  /**
   * Publish `value` under `key` and return its retirement. Last write wins, and
   * the retirement removes the entry only while it is still the one this call
   * installed: React does not order an outgoing component's cleanup before its
   * replacement's effect, so a late retirement must not clear the live entry.
   */
  register: (key: string, value: T) => () => void
  /// The value published under `key`, or undefined when nobody has.
  read: (key: string) => T | undefined
  /// Every published value, for a reader whose question spans several keys it
  /// cannot name in advance.
  values: () => IterableIterator<T>
  /// Hear every change, for a reader that is not a component.
  subscribe: (listener: () => void) => () => void
  /// Subscribe a component to every change, without reading anything itself:
  /// the caller does its own lookup afterwards, since a scan over several keys
  /// is as common here as a single read.
  useVersion: () => void
  /// Test-only: forget every registration between cases.
  reset: () => void
}

export function createKeyedRegistry<T>(): KeyedRegistry<T> {
  const entries = new Map<string, T>()
  const listeners = new Set<() => void>()
  // A monotonic counter IS the snapshot: `useSyncExternalStore` compares
  // snapshots by value, and a Map's identity either never changes or changes on
  // every read.
  let version = 0

  function publish(): void {
    version++
    for (const listener of listeners) listener()
  }

  function subscribe(listener: () => void): () => void {
    listeners.add(listener)
    return () => void listeners.delete(listener)
  }

  function snapshot(): number {
    return version
  }

  return {
    register(key, value) {
      entries.set(key, value)
      publish()
      return () => {
        if (entries.get(key) !== value) return
        entries.delete(key)
        publish()
      }
    },
    read: (key) => entries.get(key),
    values: () => entries.values(),
    subscribe,
    useVersion() {
      useSyncExternalStore(subscribe, snapshot, snapshot)
    },
    reset() {
      entries.clear()
      publish()
    },
  }
}
