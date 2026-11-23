// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { EditorContentPane } from "./EditorBody"
import type { TabBuffer } from "@/lib/editorBuffers"
import type { EditorTab } from "@/lib/editorTabs"
import { agentRoot } from "@/lib/editorRoot"

vi.mock("./CodeEditor", () => ({
  default: ({ path }: { path: string }) => {
    if (path === "broken.ts") throw new Error("boom")
    return <div>editing {path}</div>
  },
}))

afterEach(cleanup)

const tab = (path: string): EditorTab => ({ id: "t1", path, mode: "file", preview: true, dirty: false })

function pane(path: string) {
  return (
    <EditorContentPane
      root={agentRoot("s1")}
      tab={tab(path)}
      buffer={{ path, draft: "", fileError: null } as unknown as TabBuffer}
      image={false}
      fileReady
      binary={false}
      diffReady
      preview={false}
      language={undefined}
      renderPreview={() => null}
      onRetryFile={() => {}}
      onRetryDiff={() => {}}
      onDraftChange={() => {}}
      onSave={() => {}}
      onMonacoReady={() => {}}
      onEditorChange={() => {}}
      onImageDims={() => {}}
      writeBlocked={null}
    />
  )
}

describe("EditorContentPane", () => {
  it("drops an error card when the same tab moves to another file", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {})
    const { rerender } = render(pane("broken.ts"))
    expect(await screen.findByText("This part of the editor hit an error")).toBeTruthy()
    rerender(pane("fine.ts"))
    expect(await screen.findByText("editing fine.ts")).toBeTruthy()
    expect(screen.queryByText("This part of the editor hit an error")).toBeNull()
  })
})
