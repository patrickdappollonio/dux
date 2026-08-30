// The homepage tagline, and the word splitting behind its reading-order reveal.
//
// The two lines are wrapped one word per `<span>` so an IntersectionObserver can
// light them as they cross the middle of the viewport. The splitting is done
// here, at build time, rather than by rewriting text nodes in the browser: the
// shipped HTML then contains the real words, so a reader with no JavaScript, a
// crawler, and a screen reader all get the sentence rather than an empty box.
//
// A word carries its own trailing space so the line still reads normally when
// the spans are laid out inline; splitting on whitespace and rejoining with a
// separate space element would let a line break land inside a word gap.
//
// The reduced-motion and capability questions are reveal.ts's, asked here
// through its helpers rather than with a second, unguarded `matchMedia` call.
import { prefersReducedMotion, supportsReveal } from "./reveal"

/** One line of the tagline, already split for rendering. */
export interface TaglineLine {
  /** The line as one string, for anything that wants the plain sentence. */
  text: string
  /** The line's words, in reading order. */
  words: string[]
}

/**
 * The tagline itself. Two lines: what dux gives you, then what that changes
 * about your day. Kept here so the component renders data rather than prose.
 */
export const TAGLINE = [
  "Five agents on five branches, all visible at once.",
  "You stop babysitting terminals and start reviewing work.",
] as const

/**
 * Splits a line into words in reading order. Runs of whitespace collapse, and
 * empty input yields no words rather than one empty one, so an accidental blank
 * line cannot render a stray span that lights up on its own.
 */
export function splitWords(line: string): string[] {
  return line.trim().split(/\s+/).filter(Boolean)
}

/** The tagline, split, in reading order. */
export function taglineLines(lines: readonly string[] = TAGLINE): TaglineLine[] {
  return lines.map((text) => ({ text, words: splitWords(text) }))
}

/**
 * The observer inset that decides where a word lights: the top half of the
 * viewport, so a word turns as it crosses the middle on the way up.
 *
 * Deliberately a HALF rather than a narrow band across the middle. A band is the
 * obvious way to say "light it as the reader's eye passes over it", and it has a
 * bug: a fast flick can carry a word straight over a 90px band between two
 * observation frames, and that word then stays at 30% for good, in a sentence
 * the reader is meant to be able to read. Everything above the middle line
 * intersecting means a word can be reached late but never skipped.
 */
export const WORD_ROOT_MARGIN = "0px 0px -50% 0px"

/** Marks a word that has reached full colour. */
export const LIT_CLASS = "is-lit"

/** Attribute the tagline section carries, and the script looks for. */
export const TAGLINE_ATTR = "data-tagline"

/** Stamped on a section once wired, so a second call cannot observe it twice. */
export const TAGLINE_WIRED_ATTR = "data-tagline-wired"

/**
 * Wires the words of every tagline section. With no IntersectionObserver or
 * under reduced motion every word is lit at once, which is the same thing the
 * unarmed markup already shows.
 */
export function initTagline(
  doc: Document = document,
  view: Window & typeof globalThis = window,
): void {
  const sections = Array.from(doc.querySelectorAll<HTMLElement>(`[${TAGLINE_ATTR}]`)).filter(
    (section) => !section.hasAttribute(TAGLINE_WIRED_ATTR),
  )
  if (sections.length === 0) return
  sections.forEach((section) => section.setAttribute(TAGLINE_WIRED_ATTR, ""))

  const words = sections.flatMap((section) =>
    Array.from(section.querySelectorAll<HTMLElement>(".tagline-word")),
  )
  if (words.length === 0) return

  const light = (el: HTMLElement) => el.classList.add(LIT_CLASS)

  if (!supportsReveal(view) || prefersReducedMotion(view.matchMedia?.bind(view))) {
    words.forEach(light)
    return
  }

  const observer = new view.IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return
        light(entry.target as HTMLElement)
        observer.unobserve(entry.target)
      })
    },
    { threshold: 0, rootMargin: WORD_ROOT_MARGIN },
  )

  words.forEach((word) => observer.observe(word))
}
