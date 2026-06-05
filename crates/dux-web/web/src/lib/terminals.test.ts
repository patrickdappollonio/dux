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

  it("shows the TUI left-pane composite when a command is running", () => {
    // Mirrors render.rs ~691-702: "{cmd} · {label}" so the running process is
    // prominent while the terminal's stable identity stays visible.
    expect(terminalTitle(term({ foreground_cmd: "vim" }))).toBe(
      "vim · Terminal 1",
    )
  })

  it("falls back to just the label when the command normalizes to empty", () => {
    expect(terminalTitle(term({ foreground_cmd: "   " }))).toBe("Terminal 1")
  })

  it("normalizes the command inside the composite", () => {
    expect(terminalTitle(term({ foreground_cmd: "  TERM htop  " }))).toBe(
      "htop · Terminal 1",
    )
  })
})
