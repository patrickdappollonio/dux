import { describe, expect, it } from "vitest"

import { terminalTitle } from "./terminals"
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

describe("terminalTitle", () => {
  it("falls back to the label when no foreground command is running", () => {
    expect(terminalTitle(term({ foreground_cmd: null }))).toBe("Terminal 1")
  })

  it("shows the foreground command when one is running", () => {
    expect(terminalTitle(term({ foreground_cmd: "vim" }))).toBe("vim")
  })

  it("foreground command takes precedence over the label", () => {
    expect(
      terminalTitle(term({ label: "Terminal 1", foreground_cmd: "htop" })),
    ).toBe("htop")
  })

  it("trims surrounding whitespace from the foreground command", () => {
    expect(terminalTitle(term({ foreground_cmd: "  npm  " }))).toBe("npm")
  })

  it('strips a leading "TERM " prefix', () => {
    expect(terminalTitle(term({ foreground_cmd: "TERM vim" }))).toBe("vim")
  })

  it('strips a leading lowercase "term " prefix', () => {
    expect(terminalTitle(term({ foreground_cmd: "term vim" }))).toBe("vim")
  })

  it("falls back to the label when the command is empty", () => {
    expect(terminalTitle(term({ foreground_cmd: "" }))).toBe("Terminal 1")
  })

  it("falls back to the label when the command is only whitespace", () => {
    expect(terminalTitle(term({ foreground_cmd: "   " }))).toBe("Terminal 1")
  })

  it('strips the prefix even when a trailing space leaves only "TERM"', () => {
    // The TUI trims before stripping, so "TERM " becomes "TERM" (no trailing
    // space to match the "TERM " prefix) and is shown verbatim — not dropped.
    expect(terminalTitle(term({ foreground_cmd: "TERM " }))).toBe("TERM")
  })

  it("trims the command before checking the prefix", () => {
    // Surrounding whitespace is removed first, so a padded "TERM vim" still
    // matches the prefix and yields the bare command.
    expect(terminalTitle(term({ foreground_cmd: "  TERM vim  " }))).toBe("vim")
  })
})
