// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { FileInfoDialog } from "./FileInfoDialog"
import type { WorktreeEntryInfo } from "@/lib/fileInfo"

const FILE_INFO: WorktreeEntryInfo = {
  path: "src/main.rs",
  kind: "file",
  size: 2048,
  modified: "2026-02-03T04:05:06Z",
  mode: "644",
  permissions: "rw-r--r--",
  symlink_target: null,
  git: { state: "changed", staged: null, unstaged: "M" },
}

// Fake ONLY the network: the real component, the real fileApi, the real
// formatters all run.
let calls = 0

function fetchCallCount(): number {
  return calls
}

function stubFetch(
  respond: () => { status: number; body?: unknown; text?: string },
) {
  calls = 0
  vi.stubGlobal(
    "fetch",
    vi.fn(() => {
      calls += 1
      const r = respond()
      return Promise.resolve({
        ok: r.status >= 200 && r.status < 300,
        status: r.status,
        json: () => Promise.resolve(r.body),
        text: () => Promise.resolve(r.text ?? ""),
      } as unknown as Response)
    }),
  )
}

beforeEach(() => {
  stubFetch(() => ({ status: 200, body: FILE_INFO }))
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("FileInfoDialog", () => {
  it("reports every field the panel promises", async () => {
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: "src/main.rs" }}
        onClose={() => {}}
      />,
    )
    await waitFor(() => screen.getByText("rw-r--r--"))
    expect(screen.getAllByText("src/main.rs").length).toBeGreaterThan(0)
    expect(screen.getByText("File")).toBeTruthy()
    expect(screen.getByText("2.0 KiB (2,048 bytes)")).toBeTruthy()
    expect(screen.getByText("rw-r--r--")).toBeTruthy()
    expect(screen.getByText("644")).toBeTruthy()
    expect(screen.getByText("Modified, not staged")).toBeTruthy()
    // The modified row renders a real timestamp, not the placeholder.
    expect(screen.queryByText("Unknown")).toBeNull()
  })

  it("shows a folder with no size and git's folder caveat", async () => {
    stubFetch(() => ({
      status: 200,
      body: {
        ...FILE_INFO,
        path: "src",
        kind: "dir",
        size: null,
        git: { state: "not_applicable" },
      },
    }))
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: "src" }}
        onClose={() => {}}
      />,
    )
    await waitFor(() => screen.getByText("Folder"))
    expect(screen.getByText("—")).toBeTruthy()
    expect(screen.getByText(/git tracks files, not folders/i)).toBeTruthy()
  })

  it("names a symlink's target instead of following it", async () => {
    stubFetch(() => ({
      status: 200,
      body: {
        ...FILE_INFO,
        path: "link.txt",
        kind: "symlink",
        symlink_target: "../elsewhere/real.txt",
        git: { state: "clean" },
      },
    }))
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: "link.txt" }}
        onClose={() => {}}
      />,
    )
    await waitFor(() => screen.getByText("Symbolic link"))
    expect(screen.getByText("../elsewhere/real.txt")).toBeTruthy()
  })

  // The entry was ALREADY gone when the panel opened: the very first fetch is
  // a 404, and the dialog dismisses itself rather than sitting there
  // describing something that is not there.
  it("closes itself when its target was already gone at open time", async () => {
    stubFetch(() => ({ status: 404, text: "no such entry in the worktree" }))
    const onClose = vi.fn()
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: "src/gone.rs" }}
        onClose={onClose}
      />,
    )
    await waitFor(() => expect(onClose).toHaveBeenCalled())
  })

  // The journey the panel actually promises: it opened on a file that WAS
  // there, the file was deleted elsewhere (a terminal, another tab), and the
  // user came back to this tab. Regaining focus is the only revalidation
  // signal there is, so without it the panel would describe the file forever.
  it("notices on window focus that its target has since vanished", async () => {
    let gone = false
    stubFetch(() =>
      gone
        ? { status: 404, text: "no such entry in the worktree" }
        : { status: 200, body: FILE_INFO },
    )
    const onClose = vi.fn()
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: "src/main.rs" }}
        onClose={onClose}
      />,
    )
    await waitFor(() => screen.getByText("rw-r--r--"))
    expect(onClose).not.toHaveBeenCalled()

    gone = true
    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(onClose).toHaveBeenCalled())
  })

  // The same signal must not close a panel whose file is still there, or every
  // tab switch would dismiss it.
  it("stays open on window focus while its target is still there", async () => {
    const onClose = vi.fn()
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: "src/main.rs" }}
        onClose={onClose}
      />,
    )
    await waitFor(() => screen.getByText("rw-r--r--"))
    fireEvent(window, new Event("focus"))
    await waitFor(() => expect(fetchCallCount()).toBe(2))
    expect(onClose).not.toHaveBeenCalled()
    expect(screen.getByText("rw-r--r--")).toBeTruthy()
  })

  // A REFUSED path is a different answer and must stay on screen: silently
  // vanishing would hide the reason.
  it("keeps a refused path on screen with its error", async () => {
    stubFetch(() => ({
      status: 400,
      text: "refusing to access the git directory: .git/config",
    }))
    const onClose = vi.fn()
    render(
      <FileInfoDialog
        sessionId="s1"
        target={{ path: ".git/config" }}
        onClose={onClose}
      />,
    )
    await waitFor(() =>
      screen.getByText(/refusing to access the git directory/i),
    )
    expect(onClose).not.toHaveBeenCalled()
  })
})
