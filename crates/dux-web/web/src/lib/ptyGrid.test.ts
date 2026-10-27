import { describe, expect, it } from "vitest"

import {
  PTY_MAX_COLS,
  PTY_MAX_ROWS,
  PTY_MIN_COLS,
  PTY_MIN_ROWS,
  ptyGridInBounds,
} from "./ptyGrid"

describe("the PTY grid bounds", () => {
  it("are the ones the server enforces", async () => {
    const { readFileSync } = await import("node:fs")
    const rust = readFileSync(
      `${process.cwd()}/../../dux-core/src/pty.rs`,
      "utf8",
    )
    const bound = (name: string) =>
      Number(new RegExp(`pub const ${name}: u16 = (\\d+);`).exec(rust)![1])
    expect([PTY_MIN_ROWS, PTY_MIN_COLS, PTY_MAX_ROWS, PTY_MAX_COLS]).toEqual([
      bound("PTY_MIN_ROWS"),
      bound("PTY_MIN_COLS"),
      bound("PTY_MAX_ROWS"),
      bound("PTY_MAX_COLS"),
    ])
  })

  it("take every grid between them and nothing outside", () => {
    expect(ptyGridInBounds(PTY_MIN_ROWS, PTY_MIN_COLS)).toBe(true)
    expect(ptyGridInBounds(PTY_MAX_ROWS, PTY_MAX_COLS)).toBe(true)
    expect(ptyGridInBounds(0, 80)).toBe(false)
    expect(ptyGridInBounds(24, PTY_MIN_COLS - 1)).toBe(false)
    expect(ptyGridInBounds(PTY_MAX_ROWS + 1, 80)).toBe(false)
    expect(ptyGridInBounds(24, PTY_MAX_COLS + 1)).toBe(false)
    expect(ptyGridInBounds(24.5, 80)).toBe(false)
  })
})
