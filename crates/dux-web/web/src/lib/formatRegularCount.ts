// Formats a count with its noun, pluralizing regular English nouns with a
// trailing "s". An irregular plural needs its own wording at the call site.
export function formatRegularCount(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`
}
