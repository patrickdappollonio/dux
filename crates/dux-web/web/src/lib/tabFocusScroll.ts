import { REDUCED_MOTION_QUERY } from "@/hooks/use-reduced-motion"

/**
 * A tab strip's focus handler: scrolls the tab holding the newly focused
 * element fully into view, only under keyboard focus, since a click lands on
 * a tab already in view and a browser scrolls a focused tab only partly in.
 */
export function scrollFocusedTabIntoView(target: EventTarget) {
  if (!(target instanceof Element) || !target.matches(":focus-visible")) return
  const tab = target.closest('[role="tab"]')
  if (tab === null) return
  const reduced =
    typeof window.matchMedia === "function" && window.matchMedia(REDUCED_MOTION_QUERY).matches
  tab.scrollIntoView?.({ block: "nearest", inline: "nearest", behavior: reduced ? "auto" : "smooth" })
}

/**
 * A tab pill's ArrowLeft and ArrowRight: focus moves to the previous or next
 * pill of its strip and stops at the ends. Without it a browser scrolls the
 * strip sideways under the focused pill, 40px a press, until the pill is out of
 * view. The new focus then reaches the strip's own focus handler, which brings
 * it into view. Only a press on the pill itself counts: the arrows inside its
 * `⋯` menu belong to the menu.
 */
export function focusAdjacentTab(event: React.KeyboardEvent<HTMLElement>) {
  if (event.target !== event.currentTarget) return
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
  const strip = event.currentTarget.parentElement
  if (strip === null) return
  const tabs = Array.from(strip.querySelectorAll<HTMLElement>(':scope > [role="tab"]'))
  const at = tabs.indexOf(event.currentTarget)
  if (at === -1) return
  event.preventDefault()
  tabs[at + (event.key === "ArrowRight" ? 1 : -1)]?.focus()
}
