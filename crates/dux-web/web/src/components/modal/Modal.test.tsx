// @vitest-environment jsdom
import { useRef, useState, type ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { FolderPlus } from "lucide-react"

import { installBrowserHistory } from "@/test/browserHistory"

// Back reaches a modal through the store, which gives every open one a history
// entry, so the real store is loaded over a history that has one.
const browser = installBrowserHistory()
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("offline test"))),
)
await import("@/lib/store")

import { Modal } from "./index"
import type { ModalAction, ModalKind } from "./types"

const PHONE = 390
const COMPUTER = 1280

const save: ModalAction = { label: "Save", tone: "primary", onSelect: () => {} }
const remove: ModalAction = { label: "Delete", tone: "destructive", onSelect: () => {} }

// One modal of each kind with the least each kind may be written with.
function Sample({
  kind,
  onClose = () => {},
  children,
}: {
  kind: ModalKind
  onClose?: () => void
  children?: ReactNode
}) {
  const common = { open: true, onClose, title: "Sample", historyEntry: false, children }
  switch (kind) {
    case "confirm":
      return <Modal kind="confirm" {...common} cancelLabel="Cancel" confirm={remove} />
    case "form":
      return <Modal kind="form" {...common} submit={save} />
    case "picker":
      return <Modal kind="picker" {...common} />
    case "report":
      return <Modal kind="report" {...common} />
  }
}

const popup = () => document.querySelector<HTMLElement>('[data-slot="dialog-content"]')!
const footer = () => document.querySelector<HTMLElement>('[data-slot="modal-footer"]')
const closeButtons = () => screen.queryAllByRole("button", { name: "Close" })

function at(width: number) {
  vi.stubGlobal("innerWidth", width)
}

beforeEach(() => at(COMPUTER))
afterEach(() => {
  cleanup()
  void browser
})

describe("how a modal is presented", () => {
  it.each(["form", "picker", "report"] as const)(
    "covers the whole screen on a phone for a %s",
    (kind) => {
      at(PHONE)
      render(<Sample kind={kind} />)
      expect(popup().dataset.presentation).toBe("cover")
      expect(popup().className).toContain("inset-0")
    },
  )

  it("keeps a confirm a small card on a phone", () => {
    at(PHONE)
    render(<Sample kind="confirm" />)
    expect(popup().dataset.presentation).toBe("card")
    expect(popup().className).not.toContain("inset-0")
  })

  it.each(["confirm", "form", "picker", "report"] as const)(
    "floats a %s as a card on a computer",
    (kind) => {
      render(<Sample kind={kind} />)
      expect(popup().dataset.presentation).toBe("card")
    },
  )
})

describe("the way out the modal draws", () => {
  it.each(["form", "picker", "report"] as const)(
    "puts one close button in a phone cover's title row for a %s",
    (kind) => {
      at(PHONE)
      const onClose = vi.fn()
      render(<Sample kind={kind} onClose={onClose} />)
      expect(closeButtons()).toHaveLength(1)
      expect(closeButtons()[0].closest('[data-slot="modal-header"]')).not.toBeNull()
      fireEvent.click(closeButtons()[0])
      expect(onClose).toHaveBeenCalledTimes(1)
    },
  )

  it("draws no close button on a confirm or a computer's form, whose Cancel is the way out", () => {
    render(<Sample kind="form" />)
    expect(closeButtons()).toHaveLength(0)
    cleanup()
    render(<Sample kind="confirm" />)
    expect(closeButtons()).toHaveLength(0)
    cleanup()
    at(PHONE)
    render(<Sample kind="confirm" />)
    expect(closeButtons()).toHaveLength(0)
  })

  it("gives a computer's picker the corner close and a report a Close in its footer", () => {
    render(<Sample kind="picker" />)
    expect(closeButtons()).toHaveLength(1)
    expect(footer()).toBeNull()
    cleanup()
    render(<Sample kind="report" />)
    expect(closeButtons()).toHaveLength(1)
    expect(footer()!.contains(closeButtons()[0])).toBe(true)
  })
})

// The popup places focus a moment after it mounts, so each of these waits.
describe("what has focus when a modal opens", () => {
  it.each([COMPUTER, PHONE])(
    "is a confirm's safe button at width %d, whatever its body asks for",
    async (width) => {
      at(width)
      render(
        <Sample kind="confirm">
          <input aria-label="typed" autoFocus />
        </Sample>,
      )
      await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" })))
    },
  )

  it("is a form's first field on a computer, past the buttons before it", async () => {
    render(
      <Sample kind="form">
        <button type="button">shortcut</button>
        <textarea aria-label="notes" />
      </Sample>,
    )
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("notes")))
  })

  it("is the field a form names on a computer", async () => {
    function Named() {
      const second = useRef<HTMLInputElement | null>(null)
      return (
        <Modal
          kind="form"
          open
          onClose={() => {}}
          title="Sample"
          historyEntry={false}
          submit={save}
          initialFocus={second}
        >
          <input aria-label="first" />
          <input aria-label="second" ref={second} />
        </Modal>
      )
    }
    render(<Named />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("second")))
  })

  it("is a picker's search field on a computer and a report's Close", async () => {
    render(
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        search={{ value: "", onChange: () => {}, label: "Search things" }}
      >
        <button type="button">row</button>
      </Modal>,
    )
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Search things")))
    cleanup()
    render(
      <Sample kind="report">
        <button type="button">row</button>
      </Sample>,
    )
    await waitFor(() => expect(document.activeElement).toBe(closeButtons()[0]))
  })

  // The soft keyboard comes up for a focused text field, so on a phone none
  // has focus on open, not even one that asks for it.
  it("is never a text field on a phone", async () => {
    at(PHONE)
    render(
      <Sample kind="form">
        <textarea aria-label="notes" autoFocus />
      </Sample>,
    )
    await waitFor(() => expect(document.activeElement).toBe(popup()))
    cleanup()
    render(
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        search={{ value: "", onChange: () => {}, label: "Search things" }}
      />,
    )
    await waitFor(() => expect(document.activeElement).toBe(popup()))
  })

  it("is placed again when the step changes", async () => {
    function Steps() {
      const [step, setStep] = useState("list")
      return (
        <Modal
          kind="picker"
          open
          onClose={() => {}}
          title="Sample"
          historyEntry={false}
          step={step}
          search={
            step === "search"
              ? { value: "", onChange: () => {}, label: "Search things" }
              : undefined
          }
        >
          <button type="button" onClick={() => setStep("search")}>
            next
          </button>
        </Modal>
      )
    }
    render(<Steps />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByText("next")))
    fireEvent.click(screen.getByText("next"))
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Search things")))
  })
})

describe("the footer", () => {
  const button = (name: string) => screen.getByRole("button", { name })
  // The grid cell a footer button sits in: its disabled-hint wrapper.
  const cell = (name: string) => button(name).parentElement!

  it("puts a phone form's two buttons on one row, each a finger tall", () => {
    at(PHONE)
    render(<Sample kind="form" />)
    expect(footer()!.querySelector(".grid-cols-2")).not.toBeNull()
    for (const name of ["Cancel", "Save"]) {
      expect(cell(name).className).not.toContain("col-span-2")
      expect(button(name).className).toContain("min-h-11")
    }
  })

  it("gives the safe button a row of its own under the other two when there are three", () => {
    at(PHONE)
    render(
      <Modal
        kind="confirm"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        cancelLabel="Cancel"
        alternative={{ label: "Reload", tone: "plain", onSelect: () => {} }}
        confirm={{ label: "Overwrite", tone: "destructive", onSelect: () => {} }}
      />,
    )
    const order = [...footer()!.querySelectorAll("button")].map((b) => b.textContent)
    expect(order).toEqual(["Reload", "Overwrite", "Cancel"])
    expect(cell("Cancel").className).toContain("col-span-2")
    expect(cell("Overwrite").className).not.toContain("col-span-2")
  })

  it("is absent from a phone report with no action, whose title row closes it", () => {
    at(PHONE)
    render(<Sample kind="report" />)
    expect(footer()).toBeNull()
  })

  it("keeps a picker's two doors side by side on a phone", () => {
    at(PHONE)
    render(
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        doors={[
          { label: "Add a project…", icon: FolderPlus, onSelect: () => {} },
          { label: "Add an agent…", icon: FolderPlus, onSelect: () => {} },
        ]}
      />,
    )
    expect(footer()!.className).toContain("grid-cols-[1fr_auto_1fr]")
    expect(button("Add an agent…").className).toContain("min-h-11")
  })

  it("says why a disabled action is disabled", async () => {
    const onSelect = vi.fn()
    render(
      <Modal
        kind="form"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        submit={{ label: "Create", tone: "primary", onSelect, disabledReason: "Choose a folder first." }}
      />,
    )
    expect((button("Create") as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(cell("Create"))
    expect((await screen.findAllByText("Choose a folder first.")).length).toBeGreaterThan(0)
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("takes no press on a busy action", () => {
    const onSelect = vi.fn()
    render(
      <Modal
        kind="form"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        submit={{ label: "Save", tone: "primary", onSelect, busy: true }}
      />,
    )
    expect(button("Save").getAttribute("aria-busy")).toBe("true")
    fireEvent.click(button("Save"))
    expect(onSelect).not.toHaveBeenCalled()
  })
})

describe("a destructive confirm", () => {
  it("greys the page behind it, and a plain one does not", () => {
    const overlay = () => document.querySelector('[data-slot="dialog-overlay"]')!
    render(<Sample kind="confirm" />)
    expect(overlay().className).toContain("backdrop-grayscale")
    cleanup()
    render(
      <Modal
        kind="confirm"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        cancelLabel="Cancel"
        confirm={save}
      />,
    )
    expect(overlay().className).not.toContain("backdrop-grayscale")
  })
})

describe("the browser's Back", () => {
  it("closes a modal as its Cancel does, and leaves one that opened by itself alone", async () => {
    const onClose = vi.fn()
    const before = browser.index
    const view = render(
      <Modal kind="report" open onClose={onClose} title="Sample" />,
    )
    await waitFor(() => expect(browser.index).toBe(before + 1))
    act(() => browser.back())
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
    view.unmount()

    const after = browser.index
    render(<Modal kind="report" open onClose={onClose} title="Sample" historyEntry={false} />)
    await new Promise((r) => setTimeout(r, 30))
    expect(browser.index).toBe(after)
  })
})

describe("a form with unsaved edits", () => {
  function Editing({ changes, onClose }: { changes: number; onClose: () => void }) {
    return (
      <Modal
        kind="form"
        open
        onClose={onClose}
        title="Sample"
        submit={save}
        unsavedEdits={{ holder: "The sample has", changes }}
      />
    )
  }
  const asking = () => screen.queryByText("Discard unsaved changes?")

  it("asks before Cancel or Escape loses them, and closes only on Discard", async () => {
    const onClose = vi.fn()
    render(<Editing changes={2} onClose={onClose} />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(onClose).not.toHaveBeenCalled()
    await waitFor(() => expect(asking()).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
    await waitFor(() => expect(asking()).toBeNull())
    expect(onClose).not.toHaveBeenCalled()

    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" })
    await waitFor(() => expect(asking()).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }))
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it("closes at once when nothing was changed", () => {
    const onClose = vi.fn()
    render(<Editing changes={0} onClose={onClose} />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(onClose).toHaveBeenCalledTimes(1)
    expect(asking()).toBeNull()
  })

  // Back keeps asking a modal that refused it to close. One Back is one
  // question: answered with Keep editing, it is not asked again.
  it("asks once on Back, and Keep editing ends that Back", async () => {
    const onClose = vi.fn()
    const before = browser.index
    render(<Editing changes={1} onClose={onClose} />)
    await waitFor(() => expect(browser.index).toBe(before + 1))
    act(() => browser.back())
    await waitFor(() => expect(asking()).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }))
    await waitFor(() => expect(asking()).toBeNull())
    await new Promise((r) => setTimeout(r, 450))
    expect(asking()).toBeNull()
    expect(onClose).not.toHaveBeenCalled()
  })
})

describe("a busy modal", () => {
  it("refuses every way out", () => {
    const onClose = vi.fn()
    render(
      <Modal
        kind="form"
        open
        busy
        onClose={onClose}
        title="Sample"
        historyEntry={false}
        submit={save}
      />,
    )
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" })
    expect(onClose).not.toHaveBeenCalled()
  })
})

// A modal cannot be written without its kind, nor a kind without its parts;
// `tsc` fails on an unused directive, so each line below is a compile error.
export function NotAModal() {
  return (
    <>
      {/* @ts-expect-error no kind */}
      <Modal open onClose={() => {}} title="x" />
      {/* @ts-expect-error a confirm without its safe button */}
      <Modal kind="confirm" open onClose={() => {}} title="x" confirm={remove} />
      {/* @ts-expect-error a confirm without its action */}
      <Modal kind="confirm" open onClose={() => {}} title="x" cancelLabel="Cancel" />
      {/* @ts-expect-error a form without its submit */}
      <Modal kind="form" open onClose={() => {}} title="x" />
      {/* @ts-expect-error a caller-chosen class */}
      <Modal kind="report" open onClose={() => {}} title="x" className="max-h-96" />
    </>
  )
}
