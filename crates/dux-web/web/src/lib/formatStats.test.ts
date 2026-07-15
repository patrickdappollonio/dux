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
    // 1_000_000 bytes is under one MiB, so it must still read as KiB — a
    // decimal-unit formatter would wrongly say "1.0 MB".
    expect(formatBytes(1_000_000)).toBe("977 KiB")
  })
})

describe("formatCpu", () => {
  it("formats_cpu_one_decimal", () => {
    expect(formatCpu(0)).toBe("0.0%")
    expect(formatCpu(3.44)).toBe("3.4%")
    expect(formatCpu(12.15)).toBe("12.2%")
  })

  it("formats_cpu_above_one_hundred_percent", () => {
    // A multi-threaded tree spread across cores legitimately exceeds 100%.
    // Nothing may clamp it.
    expect(formatCpu(129.52382)).toBe("129.5%")
    expect(formatCpu(780)).toBe("780.0%")
  })
})
