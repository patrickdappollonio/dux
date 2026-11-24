import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

import { formatBytes, formatCpu } from "./formatStats"

describe("formatBytes", () => {
  it("formats_zero_bytes", () => {
    expect(formatBytes(0)).toBe("0 B")
  })

  it("formats_below_kib", () => {
    expect(formatBytes(512)).toBe("512 B")
    expect(formatBytes(1023)).toBe("1023 B")
  })

  it("formats_kib_range", () => {
    // The TUI prints KiB with no decimals.
    expect(formatBytes(1024)).toBe("1 KiB")
    expect(formatBytes(2048)).toBe("2 KiB")
  })

  it("formats_mib_range", () => {
    expect(formatBytes(1024 * 1024)).toBe("1.0 MiB")
    expect(formatBytes(402653184)).toBe("384.0 MiB")
  })

  it("formats_gib_range", () => {
    expect(formatBytes(1024 * 1024 * 1024)).toBe("1.0 GiB")
    expect(formatBytes(1610612736)).toBe("1.5 GiB")
  })

  it("uses_binary_units_matching_the_tui", () => {
    // 1_000_000 bytes is under one MiB, so it must still read as KiB, a
    // decimal-unit formatter would wrongly say "1.0 MB".
    expect(formatBytes(1_000_000)).toBe("977 KiB")
  })
})

describe("formatCpu", () => {
  // The other half of `the_shared_cpu_figure_fixture_holds` in dux-core's
  // resource_stats.rs: the terminal UI and the browser print the same string for the same
  // sample, rounding edges included.
  const fixture = JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "../../../../dux-core/tests/fixtures/cpu_percent_cross_language.json",
      ),
      "utf8",
    ),
  ) as { cases: { what: string; sample: number; text: string }[] }

  it("has not lost its cases", () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(12)
  })

  it.each(fixture.cases.map((c) => [c.what, c.sample, c.text] as const))(
    "prints %s as the terminal UI does",
    (_what, sample, text) => {
      expect(formatCpu(sample)).toBe(text)
    },
  )

  it("formats_cpu_above_one_hundred_percent", () => {
    // Sampling noise can nudge a figure past 100%. Nothing may clamp it.
    expect(formatCpu(129.52382)).toBe("129.5%")
    expect(formatCpu(780)).toBe("780.0%")
  })
})
