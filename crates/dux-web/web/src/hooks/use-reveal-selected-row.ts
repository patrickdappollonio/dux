import * as React from "react"

import type { SelectedTarget } from "@/lib/store"

// The attribute a sidebar row carries, set to its row key, so the reveal can
// find it.
const SIDEBAR_ROW_ATTR = "data-sidebar-row"

/** The row key of an agent's sidebar row. */
export function agentRowKey(sessionId: string): string {
  return `agent:${sessionId}`
}

/** The row key of a terminal's sidebar row. */
export function terminalRowKey(terminalId: string): string {
  return `terminal:${terminalId}`
}

/**
 * The sidebar row a selection lands on. An agent's tabs share one row, so
 * moving between them is not a new row and reveals nothing.
 */
export function selectedRowKey(target: SelectedTarget | null): string | null {
  if (target === null) return null
  return target.kind === "agent"
    ? agentRowKey(target.sessionId)
    : terminalRowKey(target.terminalId)
}

// A row that is not laid out (inside a hidden list, such as the rail while the
// sidebar is expanded) has an empty rect and cannot be revealed yet.
function isLaidOut(rect: DOMRect): boolean {
  return rect.width > 0 || rect.height > 0
}

// True once the row has been dealt with: scrolled to, or found already in view.
function reveal(container: HTMLElement, rowKey: string): boolean {
  const row = [...container.querySelectorAll(`[${SIDEBAR_ROW_ATTR}]`)].find(
    (el) => el.getAttribute(SIDEBAR_ROW_ATTR) === rowKey,
  )
  if (row === undefined) return false
  const rowRect = row.getBoundingClientRect()
  if (!isLaidOut(rowRect)) return false
  const box = container.getBoundingClientRect()
  if (rowRect.top >= box.top && rowRect.bottom <= box.bottom) return true
  // No `behavior`: the default is instant, which is how the rest of the app
  // scrolls, and what reduced motion asks for anyway.
  row.scrollIntoView({ block: "nearest" })
  return true
}

/**
 * Keeps a newly selected sidebar row in view, once per selection change.
 *
 * The returned callback ref goes on the list's scroll container, and each row
 * carries `data-sidebar-row` set to its row key. When `rowKey` changes the row
 * is scrolled into view with `block: "nearest"`, so a row already on screen
 * does not move. A row that cannot be revealed yet (an agent or terminal
 * selected before the workspace push that carries it, a row in a collapsed
 * section or behind a search, a hidden list) waits and is revealed once it
 * shows, but only until the user scrolls the list: their own scroll cancels
 * the wait, so a list they have moved is never moved for them. Once revealed
 * or cancelled the hook does nothing until the selection changes again.
 */
export function useRevealSelectedRow(
  rowKey: string | null,
): (node: HTMLElement | null) => void {
  const [container, setContainer] = React.useState<HTMLElement | null>(null)
  // Tries the waiting row again, or null when nothing is waiting.
  const retryWaiting = React.useRef<(() => void) | null>(null)

  React.useLayoutEffect(() => {
    retryWaiting.current = null
    if (rowKey === null || container === null) return
    const list = container
    const key = rowKey
    if (reveal(list, key)) return

    let waiting = true
    function stop() {
      if (!waiting) return
      waiting = false
      retryWaiting.current = null
      mutations.disconnect()
      resize?.disconnect()
      list.removeEventListener("scroll", stop)
    }
    function retry() {
      if (reveal(list, key)) stop()
    }
    // A section the list does not re-render for (a collapsed group opening)
    // adds the row without this component rendering at all.
    const mutations = new MutationObserver(retry)
    mutations.observe(list, { childList: true, subtree: true })
    // The list going from hidden to shown changes its size and nothing else.
    const resize =
      typeof ResizeObserver === "function" ? new ResizeObserver(retry) : null
    resize?.observe(list)
    // Nothing here scrolls the list while the row waits, so a scroll in that
    // time is the user's, and it ends the wait.
    list.addEventListener("scroll", stop, { passive: true })
    retryWaiting.current = retry
    return stop
  }, [container, rowKey])

  // The workspace push that carries a just-created row renders this list, so
  // the row is revealed in the same commit rather than a frame later.
  React.useLayoutEffect(() => {
    retryWaiting.current?.()
  })

  return setContainer
}
