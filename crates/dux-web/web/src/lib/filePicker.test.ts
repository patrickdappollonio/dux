// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"

import { pickFiles } from "./filePicker"

// jsdom refuses to let a test assign `input.files`, so the chosen files are
// installed the way the browser would present them and a `change` is dispatched
// by hand. The native dialog itself cannot be opened headlessly at all, which
// is stated in the plan's verification notes: what is testable here is the
// contract around it, and that is where the three real bugs live.
function makeInput(): HTMLInputElement {
  const input = document.createElement("input")
  input.type = "file"
  input.multiple = true
  document.body.appendChild(input)
  return input
}

function choose(input: HTMLInputElement, files: File[]): void {
  Object.defineProperty(input, "files", { value: files, configurable: true })
  input.dispatchEvent(new Event("change"))
}

function f(name: string): File {
  return new File(["x"], name, { type: "image/png" })
}

describe("pickFiles", () => {
  it("resolves with every chosen file, in order", async () => {
    const input = makeInput()
    const promise = pickFiles(input)
    choose(input, [f("a.png"), f("b.png")])
    await expect(promise).resolves.toEqual([
      expect.objectContaining({ name: "a.png" }),
      expect.objectContaining({ name: "b.png" }),
    ])
  })

  it("resolves empty on cancel", async () => {
    const input = makeInput()
    const promise = pickFiles(input)
    input.dispatchEvent(new Event("cancel"))
    await expect(promise).resolves.toEqual([])
  })

  it("clicks the input synchronously, inside the caller's user activation", () => {
    const input = makeInput()
    const click = vi.spyOn(input, "click")
    void pickFiles(input)
    // Not "eventually": awaiting anything first spends the activation and the
    // browser silently refuses to open the dialog.
    expect(click).toHaveBeenCalledTimes(1)
  })

  it("clears the value before every open so re-picking one file still fires", () => {
    const input = makeInput()
    const seen: string[] = []
    vi.spyOn(input, "click").mockImplementation(() => seen.push(input.value))
    void pickFiles(input)
    // Model the browser having left the previous pick's value in place.
    Object.defineProperty(input, "value", {
      value: "C:\\fakepath\\a.png",
      configurable: true,
      writable: true,
    })
    void pickFiles(input)
    expect(seen).toEqual(["", ""])
  })

  it("settles a stale pending open empty when the next one starts", async () => {
    // The `cancel` event is evergreen-only, so on an older browser a dismissed
    // dialog produces nothing at all. The next open is what settles it.
    const input = makeInput()
    const first = pickFiles(input)
    const second = pickFiles(input)
    await expect(first).resolves.toEqual([])
    choose(input, [f("later.png")])
    await expect(second).resolves.toEqual([
      expect.objectContaining({ name: "later.png" }),
    ])
  })

  it("answers each of two completed opens with its own files", async () => {
    // The listeners of a settled open are detached, so the second open's
    // `change` reaches exactly one promise rather than re-resolving the first.
    const input = makeInput()
    const first = pickFiles(input)
    choose(input, [f("a.png")])
    await expect(first).resolves.toEqual([
      expect.objectContaining({ name: "a.png" }),
    ])
    const second = pickFiles(input)
    choose(input, [f("b.png")])
    await expect(second).resolves.toEqual([
      expect.objectContaining({ name: "b.png" }),
    ])
  })
})
