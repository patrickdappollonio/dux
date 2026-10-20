// Pure helpers that say WHERE a search matched a displayed string, so the one
// highlight component (`MatchHighlight`) can light it. Every range is in code
// points (start inclusive, end exclusive), because labels and paths carry emoji
// and CJK and a UTF-16 index would land a highlight mid-character. Each surface
// picks the helper that mirrors its own filter, so what lights up is always the
// reason the row is shown.

export interface CharRange {
  start: number
  end: number
}

// The field lowered one code point at a time, with the source index of each
// lowered point: lowercasing can expand a code point, and a range found in the
// lowered text has to map back onto the text as displayed.
export function loweredWithSource(field: string): { lowered: string[]; sourceIndex: number[] } {
  const lowered: string[] = []
  const sourceIndex: number[] = []
  Array.from(field).forEach((ch, index) => {
    for (const lower of Array.from(ch.toLowerCase())) {
      lowered.push(lower)
      sourceIndex.push(index)
    }
  })
  return { lowered, sourceIndex }
}

// Sorted, with overlapping or touching ranges folded into one.
function mergeRanges(ranges: CharRange[]): CharRange[] {
  const sorted = [...ranges].sort((a, b) => a.start - b.start)
  const merged: CharRange[] = []
  for (const range of sorted) {
    const last = merged[merged.length - 1]
    if (last && range.start <= last.end) last.end = Math.max(last.end, range.end)
    else merged.push({ ...range })
  }
  return merged
}

/** Every case-insensitive occurrence of every needle in `field`, merged. The
 * needles are expected already normalized (trimmed and lowercased) the way the
 * caller's filter normalizes its query; an empty needle matches nothing. */
export function matchCharRanges(field: string, needles: readonly string[]): CharRange[] {
  const { lowered, sourceIndex } = loweredWithSource(field)
  const found: CharRange[] = []
  for (const needle of needles) {
    const q = Array.from(needle)
    if (q.length === 0) continue
    for (let start = 0; start + q.length <= lowered.length; ) {
      let hit = true
      for (let i = 0; i < q.length; i++) {
        if (lowered[start + i] !== q[i]) {
          hit = false
          break
        }
      }
      if (!hit) {
        start++
        continue
      }
      found.push({
        start: sourceIndex[start],
        end: sourceIndex[start + q.length - 1] + 1,
      })
      start += q.length
    }
  }
  return mergeRanges(found)
}

// The letter weights of cmdk's own scorer (`command-score`), which is what the
// macro list filters with: a letter right after the previous one keeps the
// score, one at the start of a word after a space or dash keeps most of it, one
// after punctuation a little less, and one in the middle of a word very little.
const SCORE_CONTINUE = 1
const SCORE_SPACE_WORD_JUMP = 0.9
const SCORE_NON_SPACE_WORD_JUMP = 0.8
const SCORE_CHARACTER_JUMP = 0.17
const PENALTY_SKIPPED = 0.999
const PENALTY_CASE_MISMATCH = 0.9999
const PENALTY_NOT_COMPLETE = 0.99
const GAP_PUNCTUATION = /[\\/_+.#"@[({&]/
const GAP_SPACE = /[\s-]/

// cmdk lowercases both sides and reads a dash as a space, so a space typed in
// the search matches either.
function cmdkNormalize(ch: string): string {
  return GAP_SPACE.test(ch) ? " " : ch.toLowerCase()
}

/** The letters a loose, cmdk-style search lit in `text`: the query as a
 * subsequence of the text, case-insensitive, placed where cmdk's scorer would
 * prefer it (word starts over mid-word letters), so "gco" on "git checkout"
 * lights the g, the c of checkout and its o. Empty when the query is empty or
 * is not a subsequence of the text: cmdk also forgives a swapped pair of
 * letters, and a row it kept that way shows nothing rather than letters that
 * are not why it matched. */
export function subsequenceMatchRanges(text: string, query: string): CharRange[] {
  const chars = Array.from(text)
  const typed = Array.from(query)
  const q = typed.map(cmdkNormalize)
  if (q.length === 0 || q.length > chars.length) return []
  const norm = chars.map(cmdkNormalize)
  const memo = new Map<string, { score: number; picks: number[] } | null>()

  // The best placement of q[qi..] in chars[from..], by cmdk's own weights.
  function best(from: number, qi: number): { score: number; picks: number[] } | null {
    if (qi === q.length) {
      return { score: from === chars.length ? 1 : PENALTY_NOT_COMPLETE, picks: [] }
    }
    const key = `${from},${qi}`
    if (memo.has(key)) return memo.get(key)!
    let winner: { score: number; picks: number[] } | null = null
    for (let at = norm.indexOf(q[qi], from); at >= 0; at = norm.indexOf(q[qi], at + 1)) {
      const rest = best(at + 1, qi + 1)
      if (!rest) continue
      let score = rest.score
      if (at === from) {
        score *= SCORE_CONTINUE
      } else if (GAP_PUNCTUATION.test(chars[at - 1])) {
        score *= SCORE_NON_SPACE_WORD_JUMP
      } else if (GAP_SPACE.test(chars[at - 1])) {
        score *= SCORE_SPACE_WORD_JUMP
      } else {
        score *= SCORE_CHARACTER_JUMP
        if (from > 0) score *= PENALTY_SKIPPED ** (at - from)
      }
      if (chars[at] !== typed[qi]) score *= PENALTY_CASE_MISMATCH
      if (!winner || score > winner.score) winner = { score, picks: [at, ...rest.picks] }
    }
    memo.set(key, winner)
    return winner
  }

  const placed = best(0, 0)
  if (!placed) return []
  return mergeRanges(placed.picks.map((at) => ({ start: at, end: at + 1 })))
}
