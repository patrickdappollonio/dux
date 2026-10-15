import * as React from "react"

/**
 * Keyboard picking for a search field over a list of rows: while the query is
 * non-empty, one row is the active row, Up and Down move it, and Enter picks it,
 * all without the field losing focus.
 *
 * `items` are the rows that can be picked, in the order they are shown, with
 * anything disabled or collapsed out of sight already left out, so the active
 * row is always one a click could have picked. It starts on the first, and any
 * change to the query starts it there again. Once the user moves it, it stays on
 * that ROW, not that position, so a list that re-sorts or refreshes under the
 * same query cannot slide a different row under Enter; only when that row
 * leaves the list does it fall back to the first.
 *
 * Ctrl-n/Ctrl-j and Ctrl-p/Ctrl-k move it as well, because the macros popover
 * (cmdk) answers to them; a browser that keeps a chord for itself (Chrome
 * keeps Ctrl-n for a new window on Linux and Windows) never hands it over.
 *
 * The field gets `fieldProps`; each row gets `rowId(key)` as its id, so the
 * field's aria-activedescendant names it, and paints itself active when its key
 * is `activeKey`. The rows are buttons rather than listbox options, so how much
 * of that a screen reader announces varies by reader.
 */
export function useSearchPick<T>({
  query,
  items,
  keyOf,
  onPick,
}: {
  query: string
  items: readonly T[]
  keyOf: (item: T) => string
  onPick: (item: T) => void
}) {
  const baseId = React.useId()
  // `key` is the row the user moved to, or null for "the first". `scroll`
  // counts the moves and query changes that should bring the active row into
  // view; a list merely changing under the user is not one of them.
  const [nav, setNav] = React.useState<{
    query: string
    key: string | null
    scroll: number
  }>({ query, key: null, scroll: 0 })
  if (nav.query !== query) {
    setNav({ query, key: null, scroll: nav.scroll + 1 })
  }
  const live = query.trim() !== "" && items.length > 0
  const found =
    nav.key === null ? -1 : items.findIndex((item) => keyOf(item) === nav.key)
  // The chosen row left the list: forget it, or it would take the highlight
  // back from wherever the user is by then if it ever returned.
  if (nav.key !== null && found < 0 && nav.query === query) {
    setNav({ ...nav, key: null })
  }
  const index = live ? Math.max(found, 0) : -1
  const active = index >= 0 ? items[index]! : null
  const activeKey = active === null ? null : keyOf(active)

  const rowId = React.useCallback(
    (key: string) => `${baseId}-${encodeURIComponent(key)}`,
    [baseId],
  )

  // Keep the active row in view as the arrows walk it past the list's edge.
  const activeKeyRef = React.useRef(activeKey)
  React.useEffect(() => {
    activeKeyRef.current = activeKey
  })
  React.useEffect(() => {
    const key = activeKeyRef.current
    if (key === null) return
    document.getElementById(rowId(key))?.scrollIntoView?.({ block: "nearest" })
  }, [nav.scroll, rowId])

  function move(by: 1 | -1) {
    const next = items[Math.max(0, Math.min(index + by, items.length - 1))]!
    setNav({ query, key: keyOf(next), scroll: nav.scroll + 1 })
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    // Mid-composition the keys belong to the IME (229 is what browsers report
    // for a key the IME took), the same guard the terminal keys use.
    if (active === null) return
    if (event.nativeEvent.isComposing || event.keyCode === 229) return
    const chord =
      event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey
    const plain =
      !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey
    const down =
      (plain && event.key === "ArrowDown") ||
      (chord && (event.key === "n" || event.key === "j"))
    const up =
      (plain && event.key === "ArrowUp") ||
      (chord && (event.key === "p" || event.key === "k"))
    let handled = true
    if (down) {
      move(1)
    } else if (up) {
      move(-1)
    } else if (plain && event.key === "Enter") {
      onPick(active)
    } else {
      handled = false
    }
    // The arrows would otherwise move the caret to either end of the text.
    if (handled) event.preventDefault()
  }

  return {
    activeKey,
    rowId,
    fieldProps: {
      onKeyDown,
      "aria-activedescendant": activeKey === null ? undefined : rowId(activeKey),
    },
  }
}
