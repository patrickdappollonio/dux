import { describe, expect, it } from "vitest"

import {
  DEFAULT_MINIMUM_PASSWORD_LENGTH,
  DEFAULT_MINIMUM_PASSWORD_SCORE,
  STRENGTH_LABELS,
  estimateStrength,
  passwordMinimums,
  passwordWrite,
} from "./passwordStrength"

describe("estimateStrength", () => {
  it("rates a password from every cracking list as weak, with a hint", async () => {
    const s = await estimateStrength("P@ssw0rd!")
    expect(s.score).toBeLessThanOrEqual(1)
    expect(s.label).toBe(STRENGTH_LABELS[s.score])
    expect(s.hint).not.toBeNull()
  })

  it("rates a long phrase of unrelated words as strong or better", async () => {
    const s = await estimateStrength("glacier mango typewriter lantern quietly")
    expect(s.score).toBeGreaterThanOrEqual(3)
  })

  it("names the five scores from weak to excellent", () => {
    expect(STRENGTH_LABELS).toEqual(["Weak", "Fair", "Good", "Strong", "Excellent"])
  })
})

describe("passwordMinimums", () => {
  it("uses what the server reports", () => {
    expect(
      passwordMinimums({ minimum_password_length: 16, minimum_password_score: 3 }),
    ).toEqual({ length: 16, score: 3 })
  })

  it("falls back to the documented defaults when the server says nothing", () => {
    expect(
      passwordMinimums({ minimum_password_length: null, minimum_password_score: null }),
    ).toEqual({ length: DEFAULT_MINIMUM_PASSWORD_LENGTH, score: DEFAULT_MINIMUM_PASSWORD_SCORE })
    expect(DEFAULT_MINIMUM_PASSWORD_LENGTH).toBe(12)
    expect(DEFAULT_MINIMUM_PASSWORD_SCORE).toBe(2)
  })
})

describe("passwordWrite", () => {
  const mins = { length: 12, score: 2 }
  const draft = (over: Partial<{ current: string; next: string; confirm: string }> = {}) => ({
    current: "",
    next: "",
    confirm: "",
    ...over,
  })

  it("writes nothing when nothing was typed", () => {
    expect(passwordWrite(draft(), { passwordSet: true, mins })).toEqual({
      kind: "none",
    })
  })

  it("asks for the current password when one is set", () => {
    const w = passwordWrite(draft({ next: "a".repeat(20), confirm: "a".repeat(20) }), {
      passwordSet: true,
      mins,
    })
    expect(w).toEqual({ kind: "invalid", message: "Type your current password to change it." })
  })

  it("refuses one shorter than the minimum, counting characters rather than bytes", () => {
    // Eleven characters, many more bytes.
    const short = "ééééééééééé"
    const w = passwordWrite(draft({ next: short, confirm: short }), {
      passwordSet: false,
      mins,
    })
    expect(w).toEqual({ kind: "invalid", message: "Use at least 12 characters." })
  })

  it("does not refuse on the meter's score: the server decides", () => {
    const pw = "aaaaaaaaaaaaaaa"
    expect(
      passwordWrite(draft({ next: pw, confirm: pw }), { passwordSet: false, mins }),
    ).toEqual({ kind: "write", next: pw })
  })

  it("refuses two new passwords that differ", () => {
    const w = passwordWrite(
      draft({ next: "glacier mango typewriter", confirm: "glacier mango typewritter" }),
      { passwordSet: false, mins },
    )
    expect(w).toEqual({ kind: "invalid", message: "The two new passwords do not match." })
  })

  it("writes a change with the current password", () => {
    const pw = "glacier mango typewriter"
    expect(
      passwordWrite(draft({ current: "old one here", next: pw, confirm: pw }), {
        passwordSet: true,
        mins,
      }),
    ).toEqual({ kind: "write", current: "old one here", next: pw })
  })

  it("writes a first password without one", () => {
    const pw = "glacier mango typewriter"
    expect(
      passwordWrite(draft({ next: pw, confirm: pw }), {
        passwordSet: false,
        mins,
      }),
    ).toEqual({ kind: "write", next: pw })
  })
})
