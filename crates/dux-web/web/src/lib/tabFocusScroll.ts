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
