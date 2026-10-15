import * as React from "react"

/**
 * Keyboard picking for a search field over a list of rows: while the query is
 * non-empty, one row is the active row, Up and Down move it, and Enter picks it,
 * all without the field losing focus.
 *
 * `items` are the rows that can be picked, in the order they are shown, with
 * anything disabled or collapsed out of sight already left out, so the active
 * row is always one a click could have picked. It starts on the first and goes
 * back there whenever the query changes. Ctrl-n/Ctrl-j and Ctrl-p/Ctrl-k move it
 * too, because the macros popover (cmdk) already answers to them.
 *
 * The field gets `fieldProps`; each row gets `rowId(key)` as its id, so the
 * field's aria-activedescendant names it, and paints itself active when its key
 * is `activeKey`.
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
  // The index rides with the query it was chosen under, so a new query starts
  // over at the first row without an effect.
  const [nav, setNav] = React.useState({ query, index: 0 })
  const live = query.trim() !== "" && items.length > 0
  const index = live
    ? Math.min(nav.query === query ? nav.index : 0, items.length - 1)
    : -1
  const active = index >= 0 ? items[index]! : null
  const activeKey = active === null ? null : keyOf(active)

  const rowId = React.useCallback(
    (key: string) => `${baseId}-${encodeURIComponent(key)}`,
    [baseId],
  )

  // Keep the active row in view as the arrows walk it past the list's edge.
  React.useEffect(() => {
    if (activeKey === null) return
    document
      .getElementById(rowId(activeKey))
      ?.scrollIntoView?.({ block: "nearest" })
  }, [activeKey, rowId])

  function move(by: 1 | -1) {
    setNav({ query, index: Math.max(0, Math.min(index + by, items.length - 1)) })
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (active === null || event.nativeEvent.isComposing) return
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
