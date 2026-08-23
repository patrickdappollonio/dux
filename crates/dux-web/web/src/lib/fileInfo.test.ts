import { describe, expect, it } from "vitest"

import { formatBytes, formatModified, gitStatusRows } from "@/lib/fileInfo"

describe("formatBytes", () => {
  it("counts small sizes in bytes, singular at one", () => {
    expect(formatBytes(0)).toBe("0 bytes")
    expect(formatBytes(1)).toBe("1 byte")
    expect(formatBytes(999)).toBe("999 bytes")
  })

  it("switches to binary units with the exact byte count alongside", () => {
    expect(formatBytes(1024)).toBe("1.0 KiB (1,024 bytes)")
    expect(formatBytes(1536)).toBe("1.5 KiB (1,536 bytes)")
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB (5,242,880 bytes)")
  })

  it("reports an absent size as a dash rather than zero", () => {
    expect(formatBytes(null)).toBe("—")
  })
})

describe("formatModified", () => {
  it("renders a parseable RFC 3339 timestamp as a locale string", () => {
    const out = formatModified("2026-02-03T04:05:06Z")
    expect(out).not.toBe("Unknown")
    expect(out).toMatch(/2026/)
  })

  it("reports an absent timestamp as Unknown", () => {
    expect(formatModified(null)).toBe("Unknown")
  })

  it("passes an unparseable value through rather than showing Invalid Date", () => {
    expect(formatModified("not a date")).toBe("not a date")
  })
})

describe("gitStatusRows", () => {
  it("says so plainly outside a repository", () => {
    expect(gitStatusRows({ state: "not_a_repository" })).toEqual([
      { label: "Not a git repository" },
    ])
  })

  it("explains that git does not track folders", () => {
    const rows = gitStatusRows({ state: "not_applicable" })
    expect(rows).toHaveLength(1)
    expect(rows[0].label).toMatch(/folder/i)
    expect(rows[0].status).toBeUndefined()
  })

  it("reports a clean file as unmodified", () => {
    expect(gitStatusRows({ state: "clean" })).toEqual([{ label: "Unmodified" }])
  })

  it("carries the raw code so the shared FileStatusIcon can render it", () => {
    expect(
      gitStatusRows({ state: "changed", staged: null, unstaged: "M" }),
    ).toEqual([{ label: "Modified, not staged", status: "M" }])
  })

  it("labels an untracked file as untracked, not as unstaged", () => {
    expect(
      gitStatusRows({ state: "changed", staged: null, unstaged: "?" }),
    ).toEqual([{ label: "Untracked", status: "?" }])
  })

  it("reports both halves when a file is staged and edited again", () => {
    expect(
      gitStatusRows({ state: "changed", staged: "A", unstaged: "M" }),
    ).toEqual([
      { label: "Added, staged", status: "A" },
      { label: "Modified, not staged", status: "M" },
    ])
  })

  // A conflict is not on one side or the other, so it must not be labelled
  // with a staged/unstaged half.
  it("labels a conflict without a staged or unstaged half", () => {
    expect(
      gitStatusRows({ state: "changed", staged: "U", unstaged: "U" }),
    ).toEqual([
      { label: "Conflict", status: "U" },
      { label: "Conflict", status: "U" },
    ])
  })

  // The status vocabulary is `fileStatusMeta`'s, once, for the whole app: an
  // unrecognised code reads as the shared neutral word and never leaks the raw
  // letter.
  it("uses the shared vocabulary for an unrecognised code instead of printing it", () => {
    const rows = gitStatusRows({ state: "changed", staged: null, unstaged: "X" })
    expect(rows).toEqual([{ label: "Changed, not staged", status: "X" }])
  })

  // The two states that exist because `git status` says nothing at all about
  // the path. Reporting either as "Unmodified" is the lie they were added for.
  it("names an ignored file as ignored rather than unmodified", () => {
    const rows = gitStatusRows({ state: "ignored" })
    expect(rows).toHaveLength(1)
    expect(rows[0].label).toMatch(/ignored/i)
    expect(rows[0].label).not.toMatch(/unmodified/i)
  })

  it("says a path in a nested repository belongs to a different one", () => {
    const rows = gitStatusRows({ state: "other_repository" })
    expect(rows).toHaveLength(1)
    expect(rows[0].label).toMatch(/different git repository/i)
    expect(rows[0].label).not.toMatch(/unmodified/i)
  })
})
