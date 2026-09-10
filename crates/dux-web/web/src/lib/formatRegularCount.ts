// Formats a count with its noun, pluralizing regular English nouns with a
// trailing "s". An irregular plural needs its own wording at the call site.
export function formatRegularCount(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`
}

// Formats a count with a noun whose plural is spelled out rather than derived,
// like "1 process" / "2 processes".
export function formatCount(n: number, singular: string, plural: string): string {
  return `${n} ${n === 1 ? singular : plural}`
}
