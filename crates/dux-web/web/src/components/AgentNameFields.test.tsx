// @vitest-environment jsdom
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { AgentNameInput } from "@/components/AgentNameFields"
import { FormGroup } from "@/components/ui/form-group"
import { sanitizeAgentName } from "@/lib/agentName"

// The dialogs hand the raw text to their state, which keeps it sanitized.
function NameField() {
  const [name, setName] = useState("")
  return (
    <FormGroup layout="stacked" control="single" label="Agent name">
      {(field) => (
        <AgentNameInput
          field={field}
          value={name}
          onChange={(raw) => setName(sanitizeAgentName(raw))}
          onSubmit={() => {}}
          generating={false}
          autoFocus={false}
        />
      )}
    </FormGroup>
  )
}

const setNativeValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!

// One key typed where the caret is, the way a browser inserts it.
async function typeKey(input: HTMLInputElement, key: string) {
  const at = input.selectionStart ?? input.value.length
  setNativeValue.call(input, input.value.slice(0, at) + key + input.value.slice(at))
  input.setSelectionRange(at + 1, at + 1)
  fireEvent.input(input)
  await act(async () => {})
}

// Animation frames run only when the test says so, as on a busy page, where a
// fast typist's next key can arrive before the frame after the last one.
const frames: FrameRequestCallback[] = []
function runFrames() {
  for (const frame of frames.splice(0)) frame(0)
}

afterEach(() => {
  cleanup()
  frames.length = 0
  vi.unstubAllGlobals()
})

describe("AgentNameInput", () => {
  it("keeps the caret after a typed space when the next key comes before the next frame", async () => {
    vi.stubGlobal("requestAnimationFrame", (frame: FrameRequestCallback) => {
      frames.push(frame)
      return frames.length
    })
    render(<NameField />)
    const input = screen.getByRole("textbox") as HTMLInputElement
    input.focus()

    for (const key of "look n") await typeKey(input, key)
    runFrames()
    for (const key of "ame here") await typeKey(input, key)
    runFrames()

    expect(input.value).toBe("look-name-here")
    expect(input.selectionStart).toBe("look-name-here".length)
  })

  it("keeps the caret where a mid-name edit put it", async () => {
    render(<NameField />)
    const input = screen.getByRole("textbox") as HTMLInputElement
    input.focus()
    for (const key of "lookhere") await typeKey(input, key)

    input.setSelectionRange(4, 4)
    await typeKey(input, " ")
    await typeKey(input, "x")

    expect(input.value).toBe("look-xhere")
    expect(input.selectionStart).toBe(6)
  })

  it("leaves an IME composition alone until it ends, then turns its spaces into dashes", async () => {
    render(<NameField />)
    const input = screen.getByRole("textbox") as HTMLInputElement
    input.focus()

    fireEvent.compositionStart(input)
    await typeKey(input, "n")
    await typeKey(input, "i")
    await typeKey(input, " ")
    await typeKey(input, "h")
    expect(input.value).toBe("ni h")

    fireEvent.compositionEnd(input, { data: "ni h" })
    await act(async () => {})
    expect(input.value).toBe("ni-h")
    expect(input.selectionStart).toBe(4)
  })
})
