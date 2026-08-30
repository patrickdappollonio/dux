// Reveal on scroll, for the sections added to the homepage landing flow.
//
// The rule the whole thing is built around: the page must be complete and
// readable with no JavaScript at all. So the markup ships fully visible, and the
// script's FIRST act is to arm a section (add `is-armed`), which is the only
// thing that lets the hidden style in global.css apply. A reader with scripts
// off, or a crawler, never sees the hidden state, because nothing ever armed it.
//
// Both halves live here, in one module, deliberately. Arming from a separate
// inline script and revealing from a bundled one splits the effect across two
// files that can fail independently: if the bundle never arrives, every armed
// section stays hidden for good. One module cannot hide what it will not also
// show.
//
// The cost of arming from a deferred module is that a section already on screen
// would blink out and travel back in, so a section is only armed when its top is
// still below the viewport at init. Everything already in view is settled where
// it is, which is also what a deep link needs: a hash pointing into a section
// must not have that section move 4rem after the browser has already scrolled
// to it.
//
// Everything above the DOM glue is pure so it can be unit tested without a
// browser (see reveal.test.ts), which is the same split ImageZoom uses.

/** Marks a section as under script control; the hidden style hangs off it. */
export const ARMED_CLASS = "is-armed"

/** Marks a section that has arrived and should be at rest. */
export const VISIBLE_CLASS = "is-visible"

/** Attribute the homepage puts on every section that reveals. */
export const REVEAL_ATTR = "data-reveal"

/** Stamped on a section once wired, so a second call cannot observe it twice. */
export const WIRED_ATTR = "data-reveal-wired"

/**
 * Fraction of a section that has to be in view before it travels in. A section
 * is tall, so waiting for half of it means the reader has already scrolled past
 * the top edge; a small slice is enough to read as "this arrived as I got here".
 */
export const REVEAL_THRESHOLD = 0.12

/** The minimum a browser needs for the effect to be worth running at all. */
export function supportsReveal(view: { IntersectionObserver?: unknown }): boolean {
  return typeof view.IntersectionObserver === "function"
}

/**
 * Whether the reader has asked for less motion. Takes the matcher rather than
 * calling `matchMedia` itself, so a test can answer either way.
 */
export function prefersReducedMotion(
  match: ((query: string) => { matches: boolean }) | undefined,
): boolean {
  if (typeof match !== "function") return false
  try {
    return match("(prefers-reduced-motion: reduce)").matches === true
  } catch {
    return false
  }
}

/** The observer configuration. One place, so the CSS and the script agree. */
export function revealObserverInit(): { threshold: number; rootMargin: string } {
  // A small bottom inset so a section starts moving as it enters rather than
  // exactly on the viewport edge, which reads as late.
  return { threshold: REVEAL_THRESHOLD, rootMargin: "0px 0px -8% 0px" }
}

/**
 * What to do with one observed section. Returning the decision rather than
 * touching the DOM is what makes the rule testable: a section is revealed once
 * and then dropped, so scrolling back up never replays it.
 */
export function revealDecision(entry: { isIntersecting: boolean }): "reveal" | "wait" {
  return entry.isIntersecting ? "reveal" : "wait"
}

/**
 * Whether a section should be hidden and travelled in, or left where it is.
 *
 * A section whose top is still below the fold has never been seen, so arming it
 * is invisible. Anything already on screen, and anything a deep link points
 * into, is settled instead: hiding it would either blink content the reader is
 * looking at or move the very thing the browser just scrolled to.
 */
export function armDecision(section: {
  top: number
  viewportHeight: number
  isHashTarget: boolean
}): "arm" | "settle" {
  if (section.isHashTarget) return "settle"
  return section.top >= section.viewportHeight ? "arm" : "settle"
}

/**
 * The element a URL fragment points at, or null when there is no fragment or
 * nothing carries that id. A malformed percent escape is treated as no target
 * rather than allowed to throw out of init.
 */
export function hashTarget(doc: Document, view: Window): Element | null {
  const hash = view.location?.hash ?? ""
  if (hash.length < 2) return null
  let id = hash.slice(1)
  try {
    id = decodeURIComponent(id)
  } catch {
    // Leave the raw fragment; an id can legitimately contain a stray percent.
  }
  return doc.getElementById(id)
}

/**
 * Wires every reveal section on the page: arms the ones still below the fold,
 * settles the rest, then observes what it armed. Safe to call more than once,
 * because a wired section is stamped and skipped, so the second call from a
 * second bundle cannot build a second observer over the same sections.
 */
export function initReveal(
  doc: Document = document,
  view: Window & typeof globalThis = window,
): void {
  const sections = Array.from(doc.querySelectorAll<HTMLElement>(`[${REVEAL_ATTR}]`)).filter(
    (section) => !section.hasAttribute(WIRED_ATTR),
  )
  if (sections.length === 0) return
  sections.forEach((section) => section.setAttribute(WIRED_ATTR, ""))

  const settle = (el: HTMLElement) => el.classList.add(VISIBLE_CLASS)

  if (!supportsReveal(view) || prefersReducedMotion(view.matchMedia?.bind(view))) {
    sections.forEach(settle)
    return
  }

  const target = hashTarget(doc, view)
  const viewportHeight = view.innerHeight ?? 0

  const armed = sections.filter((section) => {
    const decision = armDecision({
      top: section.getBoundingClientRect().top,
      viewportHeight,
      isHashTarget: target !== null && (target === section || section.contains(target)),
    })
    if (decision === "settle") {
      settle(section)
      return false
    }
    section.classList.add(ARMED_CLASS)
    return true
  })
  if (armed.length === 0) return

  const observer = new view.IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      if (revealDecision(entry) !== "reveal") return
      settle(entry.target as HTMLElement)
      observer.unobserve(entry.target)
    })
  }, revealObserverInit())

  armed.forEach((section) => observer.observe(section))
}
