import { describe, expect, it } from "vitest"

import { terminalForeground, terminalTitle } from "./terminals"
import type { TerminalView } from "./types"

function term(overrides: Partial<TerminalView>): TerminalView {
  return {
    id: "t1",
    label: "Terminal 1",
    has_output: true,
    foreground_cmd: null,
    ...overrides,
  }
}

describe("terminalForeground", () => {
  it("is null when no foreground command is running", () => {
    expect(terminalForeground(term({ foreground_cmd: null }))).toBeNull()
  })

  it("returns the running command", () => {
    expect(terminalForeground(term({ foreground_cmd: "vim" }))).toBe("vim")
  })

  it("trims surrounding whitespace", () => {
    expect(terminalForeground(term({ foreground_cmd: "  npm  " }))).toBe("npm")
  })

  it('strips a leading "TERM " prefix', () => {
    expect(terminalForeground(term({ foreground_cmd: "TERM vim" }))).toBe("vim")
  })

  it('strips a leading lowercase "term " prefix', () => {
    expect(terminalForeground(term({ foreground_cmd: "term vim" }))).toBe("vim")
  })

  it("is null when the command is empty", () => {
    expect(terminalForeground(term({ foreground_cmd: "" }))).toBeNull()
  })

  it("is null when the command is only whitespace", () => {
    expect(terminalForeground(term({ foreground_cmd: "   " }))).toBeNull()
  })

  it('keeps a bare "TERM" whose trailing space was trimmed away', () => {
    // The TUI trims before stripping, so "TERM " becomes "TERM" (no trailing
    // space to match the "TERM " prefix) and is shown verbatim — not dropped.
    expect(terminalForeground(term({ foreground_cmd: "TERM " }))).toBe("TERM")
  })

  it("trims the command before checking the prefix", () => {
    // Surrounding whitespace is removed first, so a padded "TERM vim" still
    // matches the prefix and yields the bare command.
    expect(terminalForeground(term({ foreground_cmd: "  TERM vim  " }))).toBe(
      "vim",
    )
  })
})

describe("terminalTitle", () => {
  it("shows just the label when idle", () => {
    expect(terminalTitle(term({ foreground_cmd: null }))).toBe("Terminal 1")
  })

  it("shows just the running app name when a command is running and is unique", () => {
    // When an app is in the foreground the app name is the terminal's most
    // useful label, so we surface it alone rather than composing
    // "{cmd} · {label}". The stable "Terminal N" label returns as soon as the
    // app exits and the shell is idle again.
    const t = term({ id: "term-1", foreground_cmd: "vim" })
    expect(terminalTitle(t, [t])).toBe("vim")
  })

  it("falls back to just the label when the command normalizes to empty", () => {
    const t = term({ foreground_cmd: "   " })
    expect(terminalTitle(t, [t])).toBe("Terminal 1")
  })

  it("normalizes the running app name", () => {
    const t = term({ foreground_cmd: "  TERM htop  " })
    expect(terminalTitle(t, [t])).toBe("htop")
  })

  it("appends '(#N)' when another sibling runs the same app", () => {
    // Two terminals both running vim would otherwise read identically, so we
    // disambiguate with the terminal's own counter number.
    const a = term({ id: "term-1", label: "Terminal 1", foreground_cmd: "vim" })
    const b = term({ id: "term-2", label: "Terminal 2", foreground_cmd: "vim" })
    expect(terminalTitle(a, [a, b])).toBe("vim (#1)")
    expect(terminalTitle(b, [a, b])).toBe("vim (#2)")
  })

  it("does not disambiguate when siblings run different apps", () => {
    const a = term({ id: "term-1", label: "Terminal 1", foreground_cmd: "vim" })
    const b = term({ id: "term-2", label: "Terminal 2", foreground_cmd: "htop" })
    expect(terminalTitle(a, [a, b])).toBe("vim")
    expect(terminalTitle(b, [a, b])).toBe("htop")
  })

  it("does not collide with an idle sibling", () => {
    const a = term({ id: "term-1", label: "Terminal 1", foreground_cmd: "vim" })
    const idle = term({ id: "term-2", label: "Terminal 2", foreground_cmd: null })
    expect(terminalTitle(a, [a, idle])).toBe("vim")
  })

  it("detects a collision using the normalized command", () => {
    // One reports "vim", the other "TERM vim"; both normalize to "vim", so they
    // are the same app and must be disambiguated.
    const a = term({ id: "term-1", label: "Terminal 1", foreground_cmd: "vim" })
    const b = term({
      id: "term-2",
      label: "Terminal 2",
      foreground_cmd: "TERM vim",
    })
    expect(terminalTitle(a, [a, b])).toBe("vim (#1)")
    expect(terminalTitle(b, [a, b])).toBe("vim (#2)")
  })
})
