// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"

import type { Terminal } from "@xterm/xterm"

import type { PtySocket } from "@/lib/ptySocket"

import type { ConnectionIdentity, OwnershipVerdict } from "./channels"
import type { LiveSettings, TerminalLiveSettings } from "./liveValues"
import { useUploadPipeline } from "./uploadPipeline"

// The pane's own filedrop and clipboard suites drive this end to end through
// real drag and paste events; these pin the pipeline's OWN decisions, the ones
// that are about sequencing rather than about the DOM: which sink is chosen,
// when its availability is re-asked, what the request carries, and that a batch
// always ends in exactly one report.
const { uploads, toasts } = vi.hoisted(() => ({
  uploads: [] as { name: string; opts: { pty: string; conn: string | null } }[],
  toasts: [] as { kind: string; id: string | undefined }[],
}))
vi.mock("@/lib/fileDropApi", () => ({
  FileDropApiError: class extends Error {
    status = 500
  },
  uploadDroppedFile: async (
    file: File,
    opts: { pty: string; conn: string | null },
  ) => {
    uploads.push({ name: file.name, opts })
    return {
      requested_name: file.name,
      saved_name: file.name,
      path: `/tmp/uploads/${file.name}`,
      folder_label: "~/uploads",
    }
  },
}))
vi.mock("@/lib/notify", () => ({
  notify: (_tone: string, _m: string, o?: { id?: string }) =>
    toasts.push({ kind: "final", id: o?.id }),
  notifyBusy: (_m: string, o?: { id?: string }) =>
    toasts.push({ kind: "busy", id: o?.id }),
  notifyError: (_m: string, o?: { id?: string }) =>
    toasts.push({ kind: "error", id: o?.id }),
}))
vi.mock("@/lib/attachRegistry", () => ({
  registerAttachCapability: () => () => {},
}))

class TermFake {
  pasted: string[] = []
  paste(text: string) {
    this.pasted.push(text)
  }
}

function file(name: string): File {
  return new File(["x"], name, { type: "text/plain" })
}

function setup(
  opts: {
    kind?: "agent" | "terminal"
    owner?: boolean
    open?: boolean
    composeActive?: boolean
  } = {},
) {
  const term = new TermFake()
  const pty = { isOpen: opts.open ?? true }
  const drafted: string[] = []
  const compose = document.createElement("textarea")
  let owner = opts.owner ?? true
  const ownership: OwnershipVerdict = {
    read: () => owner,
    write: (v) => {
      owner = v
    },
  }
  const connId: ConnectionIdentity = {
    read: () => "conn-1",
    write: () => {},
  }
  const live = {
    current: {
      composeActive: opts.composeActive ?? false,
      configuredDropPaste: undefined,
      launchedDropPaste: undefined,
      providerName: "claude",
      fileDropEnabled: true,
      pastedTextChars: 0,
    } as TerminalLiveSettings,
  } as LiveSettings
  const composeInputRef = {
    current: (opts.composeActive
      ? compose
      : null) as HTMLTextAreaElement | null,
  }
  const view = renderHook(() =>
    useUploadPipeline({
      id: "p1",
      kind: opts.kind ?? "agent",
      live,
      ownership,
      connId,
      termRef: { current: term as unknown as Terminal },
      ptyRef: { current: pty as unknown as PtySocket },
      composeInputRef,
      insertComposeText: (t) => drafted.push(t),
      openFilePicker: async () => [],
      isOwner: opts.owner ?? true,
      isMobile: false,
      fileDropEnabled: true,
    }),
  )
  return {
    view,
    term,
    pty,
    drafted,
    live,
    composeInputRef,
    setOwner: (v: boolean) => {
      owner = v
    },
  }
}

beforeEach(() => {
  uploads.length = 0
  toasts.length = 0
})
afterEach(() => {
  document.body.innerHTML = ""
})

describe("which sink a path lands in", () => {
  it("is the terminal while no compose box is up, and it pastes through xterm", async () => {
    const { view, term } = setup()
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    expect(term.pasted).toHaveLength(1)
    expect(term.pasted[0]).toContain("/tmp/uploads/a.txt")
  })

  it("is the DRAFT while the compose box is the typing surface, and writes nothing to the PTY", async () => {
    const { view, term, drafted } = setup({ composeActive: true })
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    expect(term.pasted).toEqual([])
    expect(drafted).toHaveLength(1)
  })
})

describe("the sequential batch", () => {
  it("finishes each upload and delivers its path before the next one starts", async () => {
    const { view, term } = setup()
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt"), file("b.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    expect(uploads.map((u) => u.name)).toEqual(["a.txt", "b.txt"])
    // An AGENT with no configured form gets the bare path; the per-provider
    // forms and the shell-safe TERMINAL form are pinned in `lib/fileDrop`.
    expect(term.pasted.map((p) => p.trim())).toEqual([
      "/tmp/uploads/a.txt",
      "/tmp/uploads/b.txt",
    ])
  })

  it("puts the spinner and the report on ONE id, so the final replaces it", async () => {
    const { view } = setup()
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt"), file("b.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    const ids = new Set(toasts.map((t) => t.id))
    expect(ids.size).toBe(1)
    expect(toasts.filter((t) => t.kind === "final")).toHaveLength(1)
  })

  it("mints a NEW id per batch, so a second drop cannot bury the first's report", async () => {
    const { view } = setup()
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    const first = toasts.at(-1)?.id
    toasts.length = 0
    await act(async () => {
      await view.result.current.runUpload(
        [file("b.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    expect(toasts.at(-1)?.id).not.toBe(first)
  })
})

describe("availability, re-asked immediately before each delivery", () => {
  it("strands the rest of a batch when ownership moves mid-upload", async () => {
    const { view, term, setOwner } = setup()
    const sink = view.result.current.activeUploadSink()
    const guarded = {
      ...sink,
      unavailable: () => {
        // Ownership moves after the first file has been delivered.
        if (term.pasted.length >= 1) setOwner(false)
        return sink.unavailable()
      },
    }
    await act(async () => {
      await view.result.current.runUpload([file("a.txt"), file("b.txt")], guarded)
    })
    // Both were SAVED; only the first was sent.
    expect(uploads).toHaveLength(2)
    expect(term.pasted).toHaveLength(1)
  })

  it("does not claim a paste when the socket has closed", async () => {
    const { view, term } = setup({ open: false })
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    expect(uploads).toHaveLength(1)
    expect(term.pasted).toEqual([])
  })

  it("strands the file rather than falling back when the compose box goes away", async () => {
    const { view, term, drafted, composeInputRef } = setup({
      composeActive: true,
    })
    const sink = view.result.current.activeUploadSink()
    composeInputRef.current = null
    await act(async () => {
      await view.result.current.runUpload([file("a.txt")], sink)
    })
    expect(drafted).toEqual([])
    // Deliberately NOT a fallback to the terminal: the toast's wording was
    // fixed when the sink was chosen at the gesture.
    expect(term.pasted).toEqual([])
  })
})

describe("the upload request", () => {
  it("carries the TERMINAL socket's own connection id, not the events one", async () => {
    const { view } = setup()
    await act(async () => {
      await view.result.current.runUpload(
        [file("a.txt")],
        view.result.current.activeUploadSink(),
      )
    })
    expect(uploads[0].opts).toEqual({ pty: "p1", conn: "conn-1" })
  })
})

describe("the drag gate", () => {
  it("refuses a drag that carries no files", () => {
    const { view } = setup()
    const e = {
      dataTransfer: { types: ["text/plain"] },
    } as unknown as React.DragEvent
    expect(view.result.current.paneAcceptsFileDrag(e)).toBe(false)
  })

  it("accepts a file drag for the input owner", () => {
    const { view } = setup()
    const e = { dataTransfer: { types: ["Files"] } } as unknown as React.DragEvent
    expect(view.result.current.paneAcceptsFileDrag(e)).toBe(true)
  })
})
