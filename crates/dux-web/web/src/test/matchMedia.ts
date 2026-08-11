// Test helper: a jsdom `window.matchMedia` stub.
//
// jsdom ships no matchMedia at all, which is why `use-mobile.ts` and
// `use-coarse-pointer.ts` both guard for it. `useIsCoarsePointer` has no
// non-matchMedia fallback (there is no other way to ask the question), so any
// test that needs the compose bar UP has to install this.
//
// It also keeps a live listener set, so a test can flip the answer and see the
// hook re-render, which is what pins the "subscribed, not read once" half of
// the hook.

type Listener = () => void

export interface MatchMediaStub {
  /** Change what a query matches and notify subscribers of that query. */
  set(query: string, matches: boolean): void
  /** Remove the stub, restoring whatever was there before. */
  restore(): void
  /** How many listeners are currently attached (pins cleanup on unmount). */
  listenerCount(): number
}

/**
 * Install a `window.matchMedia` stub whose answers come from `initial`.
 *
 * An unlisted query answers `false`, which matches jsdom's practical default
 * and keeps an unrelated media query from accidentally reading as true.
 */
export function stubMatchMedia(
  initial: Record<string, boolean> = {}
): MatchMediaStub {
  const state = new Map(Object.entries(initial))
  const listeners = new Map<string, Set<Listener>>()
  const previous = Object.getOwnPropertyDescriptor(window, "matchMedia")

  const impl = (query: string) => {
    const set = listeners.get(query) ?? new Set<Listener>()
    listeners.set(query, set)
    return {
      get matches() {
        return state.get(query) ?? false
      },
      media: query,
      addEventListener: (_: string, cb: Listener) => set.add(cb),
      removeEventListener: (_: string, cb: Listener) => set.delete(cb),
    } as unknown as MediaQueryList
  }

  Object.defineProperty(window, "matchMedia", {
    value: impl,
    configurable: true,
    writable: true,
  })

  return {
    set(query, matches) {
      state.set(query, matches)
      for (const cb of listeners.get(query) ?? []) cb()
    },
    restore() {
      if (previous) Object.defineProperty(window, "matchMedia", previous)
      else delete (window as { matchMedia?: unknown }).matchMedia
      listeners.clear()
    },
    listenerCount() {
      let n = 0
      for (const set of listeners.values()) n += set.size
      return n
    },
  }
}

/** The query `useIsCoarsePointer` subscribes to. */
export const COARSE_POINTER_QUERY = "(pointer: coarse)"

/** Install a stub reporting touch as the primary pointer. */
export function stubCoarsePointer(coarse = true): MatchMediaStub {
  return stubMatchMedia({ [COARSE_POINTER_QUERY]: coarse })
}
