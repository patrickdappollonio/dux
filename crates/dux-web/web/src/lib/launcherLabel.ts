// The words the launcher's New… button cycles through, and when. It rests on
// "New…" and now and then names what is behind it, one word at a time. The
// first word is the button's accessible name and what it shows whenever it is
// not cycling.

export const LAUNCHER_LABELS = [
  "New…",
  "New agent",
  "New terminal",
  "New project",
] as const

// How long each word holds, in order: the rest, then a short look at each.
const HOLD_MS = [8000, 2500, 2500, 2500] as const

/** How long a word takes to slide out while the next slides in. */
export const LAUNCHER_SLIDE_MS = 125

// Every word's hold plus the slide that ends it: one 16-second loop.
const SEGMENT_MS = HOLD_MS.map((hold) => hold + LAUNCHER_SLIDE_MS)
export const LAUNCHER_CYCLE_MS = SEGMENT_MS.reduce((sum, ms) => sum + ms, 0)

/**
 * Which word shows `elapsedMs` of wall-clock time into a cycle that started on
 * "New…", and how long until the next one takes over.
 */
export function launcherLabelAt(elapsedMs: number): {
  index: number
  nextInMs: number
} {
  let into = ((elapsedMs % LAUNCHER_CYCLE_MS) + LAUNCHER_CYCLE_MS) %
    LAUNCHER_CYCLE_MS
  for (let index = 0; index < SEGMENT_MS.length; index++) {
    if (into < SEGMENT_MS[index]!) {
      return { index, nextInMs: SEGMENT_MS[index]! - into }
    }
    into -= SEGMENT_MS[index]!
  }
  return { index: 0, nextInMs: SEGMENT_MS[0]! }
}
