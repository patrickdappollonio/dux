import { describe, expect, it } from "vitest"

import { FOLD_SLACK_PX, nextFoldCount, type FoldMeasure } from "./use-fold-to-fit"

// A header in fold order: Theater, Macros…, New tab…. `room` is the row the
// names have with every control folded; the names need `names` in full.
const WIDTHS = [100, 105, 110]
const NAMES = 300

function measureAt(room: number, count: number): FoldMeasure {
  let rowWidth = room
  for (let i = count; i < WIDTHS.length; i++) rowWidth -= WIDTHS[i]
  return {
    truncated: NAMES > rowWidth,
    namesWidth: Math.min(NAMES, rowWidth),
    rowWidth,
    widths: WIDTHS,
  }
}

// Every count the header passes through from `start` until a measurement
// changes nothing, the resting count last.
function settle(room: number, start: number): number[] {
  const seen = [start]
  for (;;) {
    const next = nextFoldCount(seen[seen.length - 1], measureAt(room, seen[seen.length - 1]))
    if (next === seen[seen.length - 1]) return seen
    seen.push(next)
  }
}

// The fewest folded that leave the names whole with the slack beside the
// standing controls: the one resting count for that room.
function expected(room: number): number {
  if (room >= NAMES + 315 + FOLD_SLACK_PX) return 0
  if (room >= NAMES + 215 + FOLD_SLACK_PX) return 1
  if (room >= NAMES + 110 + FOLD_SLACK_PX) return 2
  return 3
}

describe("nextFoldCount", () => {
  it("folds one more control for each measurement that finds a name cut off", () => {
    expect(nextFoldCount(0, { truncated: true, namesWidth: 0, rowWidth: 10, widths: WIDTHS })).toBe(1)
    expect(nextFoldCount(2, { truncated: true, namesWidth: 0, rowWidth: 10, widths: WIDTHS })).toBe(3)
    expect(nextFoldCount(3, { truncated: true, namesWidth: 0, rowWidth: 10, widths: WIDTHS })).toBe(3)
  })

  it("rests at one count for each room, whatever count it starts from, visiting none twice", () => {
    for (let room = 0; room <= 700; room++) {
      for (let start = 0; start <= WIDTHS.length; start++) {
        const path = settle(room, start)
        expect(new Set(path).size).toBe(path.length)
        expect(path[path.length - 1], `room ${room} from ${start}`).toBe(expected(room))
      }
    }
  })

  it("brings back one control at a time as the room grows, the last folded first", () => {
    const rests = [700, 600, 500, 400].map((room) => settle(room, 3).at(-1))
    expect(rests).toEqual([0, 1, 2, 3])
  })

  it("leaves a control folded until it has been seen on screen", () => {
    const m = { truncated: false, namesWidth: 10, rowWidth: 2000, widths: [null, 105, 110] }
    expect(nextFoldCount(1, m)).toBe(1)
  })

  it("decides nothing while the row has no width", () => {
    const m = { truncated: false, namesWidth: 0, rowWidth: 0, widths: WIDTHS }
    expect(nextFoldCount(0, m)).toBe(0)
    expect(nextFoldCount(2, m)).toBe(2)
  })
})
