import { describe, expect, it, vi } from "vitest"
import {
  classifyDroppedItems,
  editorDropDirLabel,
  editorDropToast,
  performTreeDrop,
  swallowMissedFileDrop,
} from "./editorDrop"
import { FileDropApiError } from "./fileDropApi"
import type { DropToast } from "./fileDrop"

const saved = (requestedName: string, savedName = requestedName) =>
  ({ kind: "saved", requestedName, savedName }) as const
const refused = (requestedName: string, reason: string) =>
  ({ kind: "refused", requestedName, reason }) as const

describe("editorDropDirLabel", () => {
  it("names the worktree root rather than showing an empty string", () => {
    // "" is how the root travels on the wire, and it is not a thing anybody
    // can read in a sentence.
    expect(editorDropDirLabel("")).toBe("the worktree root")
  })

  it("uses the worktree-relative path for any other folder", () => {
    expect(editorDropDirLabel("src/ui")).toBe("src/ui")
  })
})

describe("editorDropToast", () => {
  it("says what was saved and where, for one file", () => {
    const toast = editorDropToast([saved("logo.png")], "assets")
    expect(toast.tone).toBe("success")
    expect(toast.message).toBe("Saved logo.png to assets.")
  })

  it("names the folder as the worktree root for a drop on empty space", () => {
    expect(editorDropToast([saved("notes.md")], "").message).toBe(
      "Saved notes.md to the worktree root.",
    )
  })

  it("says what a renamed file is now called, and why", () => {
    // The user has to be able to find the file, and "nothing was overwritten"
    // is the whole reason the name changed.
    const toast = editorDropToast(
      [saved("notes.md", "notes-20260809-120000-1.md")],
      "docs",
    )
    expect(toast.tone).toBe("success")
    expect(toast.message).toBe(
      "Saved notes.md to docs as notes-20260809-120000-1.md, so nothing was overwritten.",
    )
  })

  it("counts a multi-file drop and still names every rename", () => {
    const toast = editorDropToast(
      [saved("a.png"), saved("b.png", "b-1.png"), saved("c.png")],
      "assets",
    )
    expect(toast.tone).toBe("success")
    expect(toast.message).toBe(
      "Saved 3 files to assets. b.png was saved as b-1.png, so nothing was overwritten.",
    )
  })

  it("warns, rather than claiming success, when part of a drop was refused", () => {
    const toast = editorDropToast(
      [saved("a.png"), refused("b.png", "the file name contains a null byte")],
      "assets",
    )
    expect(toast.tone).toBe("warning")
    expect(toast.message).toBe(
      "Saved 1 of 2 files to assets. Refused: b.png (the file name contains a null byte).",
    )
  })

  it("is an error when nothing was saved at all, and quotes the reason", () => {
    const toast = editorDropToast(
      [refused("b.png", "dux will not save a file inside the git directory")],
      ".git",
    )
    expect(toast.tone).toBe("error")
    expect(toast.message).toBe(
      "Could not save b.png: dux will not save a file inside the git directory.",
    )
  })

  it("lists the reasons when a whole multi-file drop was refused", () => {
    const toast = editorDropToast(
      [refused("a.png", "first reason"), refused("b.png", "second reason")],
      "assets",
    )
    expect(toast.tone).toBe("error")
    expect(toast.message).toBe(
      "Could not save any of the 2 dropped files. a.png (first reason), b.png (second reason).",
    )
  })

  it("caps a long refusal list rather than printing every name", () => {
    const many = Array.from({ length: 9 }, (_, i) =>
      refused(`f${i}.png`, "nope"),
    )
    const toast = editorDropToast(many, "assets")
    expect(toast.message).toContain("and 4 more")
    expect(toast.message).not.toContain("f8.png")
  })

  it("never claims a path was pasted anywhere", () => {
    // The editor drop is the DURABLE intent: it saves a file and pastes
    // nothing. Borrowing the pane drop's wording would promise a paste that
    // never happened.
    for (const toast of [
      editorDropToast([saved("a.png")], "x"),
      editorDropToast([saved("a.png"), refused("b.png", "nope")], "x"),
      editorDropToast([refused("b.png", "nope")], "x"),
    ]) {
      expect(toast.message).not.toMatch(/past|sent|terminal/i)
    }
  })
})

describe("no tree-drop report ever waits for the user", () => {
  // Every rung of this ladder is `sticky: false`, and it is a decision rather
  // than a default. A tree drop is DURABLE and VISIBLE: the file lands in the
  // folder the user pointed at, git can see it, and the tree refreshes to show
  // it. Nothing in the report is the only copy of anything, which is the bar
  // `NotifyOptions.sticky` sets. That is the whole difference from the pane
  // drop's stranded rung, where the path in the message is the only place the
  // saved file is named.
  //
  // A refusal is not sticky either, for the same reason it is not on the pane
  // path: the file was never taken from the user, so it is still on their disk
  // exactly where they dragged it from.
  it("does not pin a success, a partial refusal or a total failure", () => {
    const rungs: DropToast[] = [
      editorDropToast([saved("logo.png")], "assets"),
      editorDropToast([saved("a.png"), saved("b.png")], "assets"),
      editorDropToast([saved("a.png"), refused("b.png", "too large")], "assets"),
      editorDropToast([refused("b.png", "too large")], "assets"),
      editorDropToast(
        [refused("a.png", "too large"), refused("b.png", "too large")],
        "assets",
      ),
    ]
    for (const rung of rungs) expect(rung.sticky).toBe(false)
  })
})

describe("performTreeDrop", () => {
  function harness(
    upload: (file: File, dir: string) => Promise<{ saved_name: string }>,
  ) {
    const calls: string[] = []
    const finals: DropToast[] = []
    const deps = {
      upload: (file: File, dir: string) => {
        calls.push(`upload:${file.name}->${dir}`)
        return upload(file, dir)
      },
      revalidateDirs: vi.fn((dirs: string[]) => {
        calls.push(`revalidate:${dirs.join(",")}`)
      }),
      refreshSearchIndex: vi.fn(() => {
        calls.push("search")
        return Promise.resolve()
      }),
      reportBusy: vi.fn((m: string) => calls.push(`busy:${m}`)),
      reportFinal: vi.fn((t: DropToast) => {
        calls.push(`final:${t.tone}`)
        finals.push(t)
      }),
    }
    return { deps, calls, finals }
  }

  const ok = (name: string) => Promise.resolve({ saved_name: name })
  const f = (name: string) => new File(["x"], name)
  /// A drop carrying only files, which is what almost every test here is
  /// about. Folders get their own tests below.
  const justFiles = (...files: File[]) => ({ files, folders: [] })

  it("uploads every file into the dropped directory and then refreshes it", async () => {
    const { deps, calls, finals } = harness((file) => ok(file.name))
    await performTreeDrop("assets", justFiles(f("a.png"), f("b.png")), deps)

    expect(calls).toEqual([
      "busy:Saving a.png to assets (1 of 2)…",
      "upload:a.png->assets",
      "busy:Saving b.png to assets (2 of 2)…",
      "upload:b.png->assets",
      "revalidate:assets",
      "final:success",
      "search",
    ])
    expect(finals[0].message).toBe("Saved 2 files to assets.")
  })

  it("raises a spinner before EACH file, not once for the whole drop", async () => {
    // Two things ride on this, and only one of them is progress. The busy
    // toast's leak guard is armed for 60 seconds and is only re-armed when
    // something touches that toast id, so a single call at the start of a slow
    // or twenty-file drop had its spinner silently retired part-way through and
    // showed nothing at all until the final toast landed. Touching the id per
    // file keeps the guard alive for exactly as long as the work runs.
    const { deps, calls } = harness((file) => ok(file.name))
    const many = Array.from({ length: 5 }, (_, i) => f(`f${i}.png`))
    await performTreeDrop("docs", justFiles(...many), deps)

    const busy = calls.filter((c) => c.startsWith("busy:"))
    expect(busy).toHaveLength(5)
    expect(busy[4]).toBe("busy:Saving f4.png to docs (5 of 5)…")
    // And each one is raised BEFORE its own upload, not after it.
    expect(calls.indexOf("busy:Saving f3.png to docs (4 of 5)…")).toBeLessThan(
      calls.indexOf("upload:f3.png->docs"),
    )
  })

  it("names the single file it is saving in the busy message", async () => {
    const { deps, calls } = harness((file) => ok(file.name))
    await performTreeDrop("", justFiles(f("logo.png")), deps)
    expect(calls[0]).toBe("busy:Saving logo.png to the worktree root…")
  })

  it("keeps going after one file is refused and reports both", async () => {
    // A refusal is per-file (an unusable name, a symlink in the way), so
    // abandoning the rest of the drop would lose files that were fine.
    const { deps, finals } = harness((file) =>
      file.name === "bad.png"
        ? Promise.reject(new FileDropApiError("the file name is unusable", 400))
        : ok(file.name),
    )
    await performTreeDrop("docs", justFiles(f("bad.png"), f("good.png")), deps)

    expect(deps.revalidateDirs).toHaveBeenCalledWith(["docs"])
    expect(finals[0].tone).toBe("warning")
    expect(finals[0].message).toContain("Saved 1 of 2 files to docs")
    expect(finals[0].message).toContain("bad.png (the file name is unusable)")
  })

  it("does not refresh anything when nothing was saved", async () => {
    // Refreshing would be a round trip that can only report what is already
    // on screen, and it would imply something changed.
    const { deps, finals } = harness(() =>
      Promise.reject(new FileDropApiError("no", 400)),
    )
    await performTreeDrop("docs", justFiles(f("a.png")), deps)

    expect(deps.revalidateDirs).not.toHaveBeenCalled()
    expect(deps.refreshSearchIndex).not.toHaveBeenCalled()
    expect(finals[0].tone).toBe("error")
  })

  it("reports the collision rename the server chose", async () => {
    const { deps, finals } = harness(() =>
      Promise.resolve({ saved_name: "notes-20260809-1.md" }),
    )
    await performTreeDrop("", justFiles(f("notes.md")), deps)
    expect(finals[0].message).toBe(
      "Saved notes.md to the worktree root as notes-20260809-1.md, so nothing was overwritten.",
    )
  })

  it("turns an unexpected throw into a refusal rather than losing the drop", async () => {
    // A non-FileDropApiError (a bug, an aborted fetch) must still produce one
    // toast; a silent rejection is a drop that appears to have done nothing.
    const { deps, finals } = harness(() => Promise.reject(new Error("boom")))
    await performTreeDrop("", justFiles(f("a.png")), deps)
    expect(finals[0].tone).toBe("error")
    expect(finals[0].message).toContain("boom")
  })

  // Dropping a FOLDER on a file tree is a natural gesture and dux does not
  // take one. What it must never do is stay quiet about it, or blame the
  // network for it.
  describe("a folder in the drop", () => {
    it("is refused by name, and nothing is uploaded for it", async () => {
      const { deps, calls, finals } = harness((file) => ok(file.name))
      await performTreeDrop("assets", { files: [], folders: ["icons"] }, deps)

      expect(calls.some((c) => c.startsWith("upload:"))).toBe(false)
      expect(finals[0].tone).toBe("error")
      expect(finals[0].message).toBe(
        "Could not save icons: dux cannot take a folder, drop its files.",
      )
    })

    it("still saves the files dropped alongside it, in one toast", async () => {
      const { deps, calls, finals } = harness((file) => ok(file.name))
      await performTreeDrop(
        "assets",
        { files: [f("logo.png")], folders: ["icons"] },
        deps,
      )

      expect(calls).toContain("upload:logo.png->assets")
      expect(calls.filter((c) => c.startsWith("final:"))).toHaveLength(1)
      expect(finals[0].tone).toBe("warning")
      expect(finals[0].message).toContain("Saved 1 of 2 files to assets")
      expect(finals[0].message).toContain(
        "icons (dux cannot take a folder, drop its files)",
      )
    })
  })

  it("says so when a drop delivered nothing at all", async () => {
    // The other browser shape for a dropped folder: no files and no items.
    // Without a report here, letting go of a folder looks exactly like letting
    // go of nothing.
    const { deps, calls, finals } = harness((file) => ok(file.name))
    await performTreeDrop("", justFiles(), deps)

    expect(calls.some((c) => c.startsWith("upload:"))).toBe(false)
    expect(deps.revalidateDirs).not.toHaveBeenCalled()
    expect(finals[0].tone).toBe("error")
    expect(finals[0].message).toBe(
      "Nothing came through in that drop. If you dropped a folder, drop the " +
        "files inside it instead.",
    )
    // Nothing was taken and nothing was saved, so there is nothing to recover
    // and no reason to make the user dismiss it.
    expect(finals[0].sticky).toBe(false)
  })
})

describe("classifyDroppedItems", () => {
  const f = (name: string) => new File(["x"], name)
  const fileItem = (name: string, isDirectory: boolean) => ({
    kind: "file",
    webkitGetAsEntry: () => ({ isDirectory, name }),
  })

  it("keeps plain files as files", () => {
    const files = [f("a.png"), f("b.png")]
    expect(
      classifyDroppedItems(files, [
        fileItem("a.png", false),
        fileItem("b.png", false),
      ]),
    ).toEqual({ files, folders: [] })
  })

  it("pulls a directory out of the file list and names it", () => {
    // The shape that made a folder drop look like a transport failure: the
    // directory rides in `files` as an entry whose read throws, so uploading it
    // reports a network-shaped error for something the user did on purpose.
    const logo = f("logo.png")
    const asDir = f("icons")
    expect(
      classifyDroppedItems(
        [logo, asDir],
        [fileItem("logo.png", false), fileItem("icons", true)],
      ),
    ).toEqual({ files: [logo], folders: ["icons"] })
  })

  it("matches folders by NAME, so a non-file item cannot shift the pairing", () => {
    // `items` and `files` only line up when every item is a file. A drag
    // carrying a text item alongside the files shifts one list and not the
    // other, and an index-based pairing would then drop the wrong entry.
    const logo = f("logo.png")
    const asDir = f("icons")
    expect(
      classifyDroppedItems(
        [logo, asDir],
        [
          { kind: "string" },
          fileItem("icons", true),
          fileItem("logo.png", false),
        ],
      ),
    ).toEqual({ files: [logo], folders: ["icons"] })
  })

  it("treats everything as a file when the browser offers no entry API", () => {
    // Degrading to today's behaviour, rather than refusing legitimate files.
    const files = [f("a.png")]
    expect(classifyDroppedItems(files, [{ kind: "file" }])).toEqual({
      files,
      folders: [],
    })
    expect(classifyDroppedItems(files, undefined)).toEqual({
      files,
      folders: [],
    })
  })

  it("leaves an item that throws when asked to describe itself", () => {
    const files = [f("a.png")]
    expect(
      classifyDroppedItems(files, [
        {
          kind: "file",
          webkitGetAsEntry: () => {
            throw new Error("nope")
          },
        },
      ]),
    ).toEqual({ files, folders: [] })
  })

  it("reports the empty shape as neither files nor folders", () => {
    expect(classifyDroppedItems([], [])).toEqual({ files: [], folders: [] })
  })
})

describe("swallowMissedFileDrop", () => {
  // The browser's default action for a dropped file is to NAVIGATE to it, so a
  // drag aimed at the file tree that lands a few pixels off would throw the
  // editor away and take every unsaved buffer with it.
  const event = (types: string[]) => {
    const e = {
      dataTransfer: { types, dropEffect: "copy" },
      preventDefault: vi.fn(),
    }
    return e
  }

  it("cancels a file drag and says the area takes nothing", () => {
    const e = event(["Files"])
    expect(swallowMissedFileDrop(e)).toBe(true)
    expect(e.preventDefault).toHaveBeenCalled()
    expect(e.dataTransfer.dropEffect).toBe("none")
  })

  it("leaves an in-app drag entirely alone", () => {
    const e = event(["text/plain"])
    expect(swallowMissedFileDrop(e)).toBe(false)
    expect(e.preventDefault).not.toHaveBeenCalled()
    expect(e.dataTransfer.dropEffect).toBe("copy")
  })

  it("survives an event with no dataTransfer", () => {
    const e = { dataTransfer: null, preventDefault: vi.fn() }
    expect(swallowMissedFileDrop(e)).toBe(false)
    expect(e.preventDefault).not.toHaveBeenCalled()
  })
})
