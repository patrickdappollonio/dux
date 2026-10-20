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

// The field lowered the way a filter's own `toLowerCase()` lowers it, as code
// points, with the source index of each one. Lowercasing a whole string can
// expand a code point ("İ" becomes two) and can depend on context (a final "Σ"
// becomes "ς", not "σ"), so the lowered points come from the whole string and
// the source of each from lowering the field one code point at a time, which
// gives the same lengths.
export function loweredWithSource(field: string): { lowered: string[]; sourceIndex: number[] } {
  const sourceIndex: number[] = []
  Array.from(field).forEach((ch, index) => {
    for (let n = Array.from(ch.toLowerCase()).length; n > 0; n--) sourceIndex.push(index)
  })
  const lowered = Array.from(field.toLowerCase())
  // Should a fold ever change length with its context, the per-point lowering
  // is the one whose indices are known to be right.
  if (lowered.length !== sourceIndex.length) {
    return {
      lowered: Array.from(field).flatMap((ch) => Array.from(ch.toLowerCase())),
      sourceIndex,
    }
  }
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

/** Every case-insensitive occurrence of every needle in `field`, overlapping
 * ones included, merged. The needles are expected already normalized (trimmed
 * and lowercased) the way the caller's filter normalizes its query; an empty
 * needle matches nothing. */
export function matchCharRanges(field: string, needles: readonly string[]): CharRange[] {
  const { lowered, sourceIndex } = loweredWithSource(field)
  const found: CharRange[] = []
  for (const needle of needles) {
    const q = Array.from(needle)
    if (q.length === 0) continue
    for (let start = 0; start + q.length <= lowered.length; start++) {
      let hit = true
      for (let i = 0; i < q.length; i++) {
        if (lowered[start + i] !== q[i]) {
          hit = false
          break
        }
      }
      if (hit) {
        found.push({
          start: sourceIndex[start],
          end: sourceIndex[start + q.length - 1] + 1,
        })
      }
    }
  }
  return mergeRanges(found)
}

// cmdk's scorer (`command-score` as the installed cmdk ships it), ported so
// the macro list can light the letters it matched rather than guess at them.
// The weights and the walk are cmdk's, including where it skips a step; the
// port only also records which letters each step consumed. Like cmdk it walks
// UTF-16 units of the lowered text.
const SCORE_CONTINUE = 1
const SCORE_SPACE_WORD_JUMP = 0.9
const SCORE_NON_SPACE_WORD_JUMP = 0.8
const SCORE_CHARACTER_JUMP = 0.17
const SCORE_TRANSPOSITION = 0.1
const PENALTY_SKIPPED = 0.999
const PENALTY_CASE_MISMATCH = 0.9999
const PENALTY_NOT_COMPLETE = 0.99
const IS_GAP = /[\\/_+.#"@[({&]/
const COUNT_GAPS = /[\\/_+.#"@[({&]/g
const IS_SPACE = /[\s-]/
const COUNT_SPACES = /[\s-]/g

function cmdkFormat(text: string): string {
  return text.toLowerCase().replace(COUNT_SPACES, " ")
}

interface Placed {
  score: number
  // Indices into the lowered text.
  picks: number[]
}

function commandScore(text: string, query: string): Placed {
  const lowerText = cmdkFormat(text)
  const lowerQuery = cmdkFormat(query)
  const memo = new Map<string, Placed>()

  function walk(textIndex: number, queryIndex: number): Placed {
    if (queryIndex === query.length) {
      return {
        score: textIndex === text.length ? SCORE_CONTINUE : PENALTY_NOT_COMPLETE,
        picks: [],
      }
    }
    const key = `${textIndex},${queryIndex}`
    const seen = memo.get(key)
    if (seen) return seen
    const queryChar = lowerQuery.charAt(queryIndex)
    let best: Placed = { score: 0, picks: [] }
    for (
      let index = lowerText.indexOf(queryChar, textIndex);
      index >= 0;
      index = lowerText.indexOf(queryChar, index + 1)
    ) {
      const rest = walk(index + 1, queryIndex + 1)
      let score = rest.score
      let picks = [index, ...rest.picks]
      if (score > best.score) {
        if (index === textIndex) {
          score *= SCORE_CONTINUE
        } else if (IS_GAP.test(text.charAt(index - 1))) {
          score *= SCORE_NON_SPACE_WORD_JUMP
          const breaks = text.slice(textIndex, index - 1).match(COUNT_GAPS)
          if (breaks && textIndex > 0) score *= PENALTY_SKIPPED ** breaks.length
        } else if (IS_SPACE.test(text.charAt(index - 1))) {
          score *= SCORE_SPACE_WORD_JUMP
          const breaks = text.slice(textIndex, index - 1).match(COUNT_SPACES)
          if (breaks && textIndex > 0) score *= PENALTY_SKIPPED ** breaks.length
        } else {
          score *= SCORE_CHARACTER_JUMP
          if (textIndex > 0) score *= PENALTY_SKIPPED ** (index - textIndex)
        }
        if (text.charAt(index) !== query.charAt(queryIndex)) score *= PENALTY_CASE_MISMATCH
      }
      // A swapped pair takes the letter before this one as the next query
      // letter; a doubled query letter is read once.
      const swapped =
        score < SCORE_TRANSPOSITION &&
        lowerText.charAt(index - 1) === lowerQuery.charAt(queryIndex + 1)
      const doubled =
        lowerQuery.charAt(queryIndex + 1) === lowerQuery.charAt(queryIndex) &&
        lowerText.charAt(index - 1) !== lowerQuery.charAt(queryIndex)
      if (swapped || doubled) {
        const skip = walk(index + 1, queryIndex + 2)
        if (skip.score * SCORE_TRANSPOSITION > score) {
          score = skip.score * SCORE_TRANSPOSITION
          picks = swapped ? [index - 1, index, ...skip.picks] : [index, ...skip.picks]
        }
      }
      if (score > best.score) best = { score, picks }
    }
    memo.set(key, best)
    return best
  }

  return walk(0, 0)
}

/** The letters the macro list's own match lit in `text` for its search
 * `query`, by cmdk's scorer: "gco" on "git checkout" lights the g, the c of
 * checkout and its o, and the swapped pair in "rnu" lights "Run". Empty when
 * cmdk would not keep the row, and when its lowered text does not line up with
 * the text shown, where the letters could not be placed truthfully. */
export function cmdkMatchRanges(text: string, query: string): CharRange[] {
  // cmdk trims an item's value and scores the search as typed.
  if (query.length === 0 || text !== text.trim()) return []
  const placed = commandScore(text, query)
  if (placed.score <= 0) return []
  // Each lowered UTF-16 unit, back to the code point shown.
  const unitSource: number[] = []
  Array.from(text).forEach((ch, index) => {
    for (let n = ch.toLowerCase().length; n > 0; n--) unitSource.push(index)
  })
  if (unitSource.length !== cmdkFormat(text).length) return []
  return mergeRanges(
    placed.picks
      .filter((at) => at >= 0 && at < unitSource.length)
      .map((at) => ({ start: unitSource[at], end: unitSource[at] + 1 })),
  )
}
