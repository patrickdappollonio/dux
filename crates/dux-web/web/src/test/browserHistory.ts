// Test helper: a browser history with the Navigation API, for jsdom suites that
// render a dialog over the real store and press the browser's Back.
//
// jsdom has no Navigation API, and its own history runs traversals as later
// tasks with no entry keys, so the store's dialog entries cannot be exercised on
// it. This replaces `history` and `window.navigation` with a model of the real
// thing: one key per entry slot, a push drops everything ahead, and Back or
// Forward moves the cursor and THEN fires popstate on the window, which is where
// the store listens. The address is left to jsdom's own `location`: a dialog's
// entry never changes it.
//
// Install it before the store is imported, so the store's first look at the
// history is this one.

interface Entry {
  url: string
  state: unknown
  key: string
}

export interface BrowserHistory {
  /** The browser's Back button. */
  back(): void
  /** The browser's Forward button. */
  forward(): void
  /** How many entries there are, as `history.length` reports it. */
  readonly length: number
  /** Where the cursor is. */
  readonly index: number
  /** The `history.state` of every entry. */
  states(): unknown[]
}

export function installBrowserHistory(): BrowserHistory {
  let nextKey = 0
  const mint = () => `k${++nextKey}`
  let entries: Entry[] = [{ url: "/", state: null, key: mint() }]
  let index = 0

  const popstate = () => {
    window.dispatchEvent(new PopStateEvent("popstate", { state: entries[index].state }))
  }
  const land = (target: number) => {
    if (target < 0 || target >= entries.length || target === index) return
    index = target
    popstate()
  }

  const history = {
    get state() {
      return entries[index].state
    },
    get length() {
      return entries.length
    },
    pushState(state: unknown, _title: string, url?: string) {
      entries = entries.slice(0, index + 1)
      entries.push({ url: String(url ?? entries[index].url), state, key: mint() })
      index = entries.length - 1
    },
    replaceState(state: unknown, _title: string, url?: string) {
      entries[index] = { ...entries[index], url: String(url ?? entries[index].url), state }
    },
    back: () => land(index - 1),
    forward: () => land(index + 1),
    go: (delta: number) => land(index + delta),
  }

  const navigation = {
    transition: null,
    get currentEntry() {
      return { key: entries[index].key, index, url: entries[index].url }
    },
    entries() {
      return entries.map((entry, i) => ({ key: entry.key, index: i, url: entry.url }))
    },
    traverseTo(key: string) {
      const target = entries.findIndex((entry) => entry.key === key)
      const committed =
        target < 0 ? Promise.reject(new Error("InvalidStateError")) : Promise.resolve()
      committed.catch(() => {})
      if (target >= 0) land(target)
      return { committed, finished: committed }
    },
  }

  Object.defineProperty(globalThis, "history", { value: history, configurable: true, writable: true })
  Object.defineProperty(window, "navigation", { value: navigation, configurable: true })

  return {
    back: history.back,
    forward: history.forward,
    get length() {
      return entries.length
    },
    get index() {
      return index
    },
    states: () => entries.map((entry) => entry.state),
  }
}
