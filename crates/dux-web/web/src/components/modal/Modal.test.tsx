// @vitest-environment jsdom
import { useRef, useState, type ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { FolderPlus } from "lucide-react"

import { PAGE_LAYER_ATTR } from "@/lib/pageLayer"
import { agentRowKey, registerPaneCover } from "@/lib/paneCover"
import { chip } from "@/lib/prose"

import { installBrowserHistory } from "@/test/browserHistory"
import { describeModalBack } from "@/test/modalBackCases"

// Back reaches a modal through the store, which gives every open one a history
// entry, so the real store is loaded over a history that has one.
const browser = installBrowserHistory()
const offline = vi.fn(() => Promise.reject(new Error("offline test")))
vi.stubGlobal("fetch", offline)
await import("@/lib/store")

import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog"

import { useRowChangeReason } from "@/hooks/use-agent-active-elsewhere"

import { Modal, useModalLifetime } from "./index"
import { RowReasonContext } from "./rowReason"
import { closeReasonFrom } from "./closeReason"
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

// As a browser does: the width changes, then `resize` says so.
function at(width: number) {
  vi.stubGlobal("innerWidth", width)
  act(() => {
    window.dispatchEvent(new Event("resize"))
  })
}

// The pointer is asked through `matchMedia`, which jsdom lacks.
function pointer(kind: "fine" | "coarse") {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: kind === "coarse" && query === "(pointer: coarse)",
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
}

beforeEach(() => {
  at(COMPUTER)
  pointer("fine")
})
afterEach(() => {
  cleanup()
  // The width and the pointer each test set, and nothing the store relies on.
  vi.unstubAllGlobals()
  vi.stubGlobal("fetch", offline)
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

describe("the rows a phone toast starts below", () => {
  function Rows() {
    return (
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        description="What this is for"
        historyEntry={false}
        search={{ value: "", onChange: () => {}, label: "Search things" }}
        toolbar={<button type="button">Sort</button>}
      />
    )
  }
  const marked = (el: HTMLElement) => el.closest("[data-toast-clear]") !== null

  it("are every row above a cover's body", () => {
    at(PHONE)
    render(<Rows />)
    expect(marked(screen.getByRole("heading", { name: "Sample" }))).toBe(true)
    expect(marked(screen.getByText("What this is for"))).toBe(true)
    expect(marked(screen.getByLabelText("Search things"))).toBe(true)
    expect(marked(screen.getByRole("button", { name: "Sort" }))).toBe(true)
  })

  it("are none on a card, whose rows hang from no edge", () => {
    render(<Rows />)
    expect(popup().querySelector("[data-toast-clear]")).toBeNull()
  })
})

// A resize or a rotation across the layout switch changes the presentation
// under an open modal; what the caller put in it must not start over.
describe("a layout switch under an open modal", () => {
  function Counter() {
    const [count, setCount] = useState(0)
    return (
      <button type="button" onClick={() => setCount(count + 1)}>
        pressed {count}
      </button>
    )
  }
  function Flipping({ kind }: { kind: "form" | "picker" | "report" }) {
    const common = { open: true, onClose: () => {}, title: "Sample", historyEntry: false }
    if (kind === "form") {
      return (
        <Modal kind="form" {...common} submit={save}>
          <Counter />
        </Modal>
      )
    }
    if (kind === "report") {
      return (
        <Modal kind="report" {...common}>
          <Counter />
        </Modal>
      )
    }
    return (
      <Modal
        kind="picker"
        {...common}
        steadyHeight
        search={{ value: "", onChange: () => {}, label: "Search things" }}
      >
        <Counter />
      </Modal>
    )
  }

  it.each(["form", "picker", "report"] as const)("keeps a %s's body and its state", (kind) => {
    const view = render(<Flipping kind={kind} />)
    fireEvent.click(screen.getByText("pressed 0"))
    const body = screen.getByText("pressed 1")
    const search = screen.queryByLabelText("Search things")
    at(PHONE)
    view.rerender(<Flipping kind={kind} />)
    expect(popup().dataset.presentation).toBe("cover")
    expect(screen.getByText("pressed 1")).toBe(body)
    expect(screen.queryByLabelText("Search things")).toBe(search)
    at(COMPUTER)
    view.rerender(<Flipping kind={kind} />)
    expect(popup().dataset.presentation).toBe("card")
    expect(screen.getByText("pressed 1")).toBe(body)
  })
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
      expect(onClose).toHaveBeenCalledWith("close-control")
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

  it("gives a report whose close is the corner that close alone, and focuses it on a computer", async () => {
    const onClose = vi.fn()
    const report = (
      <Modal
        kind="report"
        open
        onClose={onClose}
        title="Sample"
        historyEntry={false}
        closeAt="corner"
        actions={[save]}
      />
    )
    render(report)
    expect(closeButtons()).toHaveLength(1)
    expect(footer()!.contains(closeButtons()[0])).toBe(false)
    expect(footer()!.querySelectorAll("button")).toHaveLength(1)
    await waitFor(() => expect(document.activeElement).toBe(closeButtons()[0]))
    fireEvent.click(closeButtons()[0])
    expect(onClose).toHaveBeenCalledWith("close-control")
    cleanup()
    at(PHONE)
    render(report)
    expect(closeButtons()).toHaveLength(1)
    expect(closeButtons()[0].closest('[data-slot="modal-header"]')).not.toBeNull()
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

  const searchable = (
    <Modal
      kind="picker"
      open
      onClose={() => {}}
      title="Sample"
      historyEntry={false}
      search={{ value: "", onChange: () => {}, label: "Search things" }}
    />
  )

  // The soft keyboard comes up for a focused text field, so under a finger
  // none has focus on open, not even one that asks for it, at any width.
  it.each([PHONE, COMPUTER])("is never a text field under a coarse pointer at width %d", async (width) => {
    at(width)
    pointer("coarse")
    render(
      <Sample kind="form">
        <textarea aria-label="notes" autoFocus />
      </Sample>,
    )
    await waitFor(() => expect(document.activeElement).toBe(popup()))
    cleanup()
    render(searchable)
    await waitFor(() => expect(document.activeElement).toBe(popup()))
  })

  it("is still the search field in a narrow window with a mouse", async () => {
    at(PHONE)
    render(searchable)
    expect(popup().dataset.presentation).toBe("cover")
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Search things")))
  })

  // Nothing is typed into a read-only field, so no keyboard comes up for it.
  it("is a read-only text area a report names, under a coarse pointer too", async () => {
    pointer("coarse")
    function Copied() {
      const text = useRef<HTMLTextAreaElement | null>(null)
      return (
        <Modal kind="report" open onClose={() => {}} title="Sample" historyEntry={false} initialFocus={text}>
          <textarea aria-label="copied" readOnly ref={text} defaultValue="abc" />
        </Modal>
      )
    }
    render(<Copied />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("copied")))
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

  // Cancel is on screen from the first frame and loses nothing, where the
  // body's first control may be a row still loading.
  it("is a form's Cancel on a computer when it has no field", async () => {
    render(<Sample kind="form" />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" })))
  })

  it("is a picker's corner close on a computer when it has nothing else", async () => {
    render(<Sample kind="picker" />)
    await waitFor(() => expect(document.activeElement).toBe(closeButtons()[0]))
  })
})

describe("a picker's description", () => {
  it.each([PHONE, COMPUTER])("comes before its search at width %d", (width) => {
    at(width)
    render(
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        description="What this is for"
        historyEntry={false}
        search={{ value: "", onChange: () => {}, label: "Search things" }}
      />,
    )
    const order = screen
      .getByText("What this is for")
      .compareDocumentPosition(screen.getByLabelText("Search things"))
    expect(order & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })
})

// The body's scroll position is kept across a switch between card and cover;
// the scroll events the browser fires as it clamps the other presentation are
// not the person's and change nothing.
describe("the body's scroll across a switch between card and cover", () => {
  const body = () => document.querySelector<HTMLElement>('[data-slot="modal-body"]')!
  const scrollTo = (top: number) => {
    body().scrollTop = top
    fireEvent.scroll(body())
  }

  it("comes back where the person left it, whatever the clamping in between", () => {
    render(<Sample kind="form">{"A long body"}</Sample>)
    scrollTo(120)
    at(PHONE)
    expect(body().scrollTop).toBe(120)
    // The cover has less to scroll, and the browser clamps it.
    scrollTo(0)
    at(COMPUTER)
    expect(body().scrollTop).toBe(120)
  })

  it("keeps where the person scrolled to after a switch once they scroll again", () => {
    render(<Sample kind="form">{"A long body"}</Sample>)
    scrollTo(120)
    at(PHONE)
    fireEvent.wheel(body())
    scrollTo(40)
    at(COMPUTER)
    expect(body().scrollTop).toBe(40)
  })
})

describe("a card that changes step", () => {
  // A card's height is its step's own, unless a height is held on it.
  const natural: Record<string, number> = { long: 600, short: 300 }
  let height: ReturnType<typeof vi.spyOn> | null = null
  beforeEach(() => {
    height = vi
      .spyOn(HTMLElement.prototype, "offsetHeight", "get")
      .mockImplementation(function (this: HTMLElement) {
        if (this.dataset.slot !== "dialog-content") return 0
        const held = parseFloat(this.style.height)
        if (held > 0) return held
        return natural[this.querySelector<HTMLElement>("[data-step]")?.dataset.step ?? ""] ?? 0
      })
  })
  afterEach(() => height?.mockRestore())

  function Steps({ first }: { first: string }) {
    const [step, setStep] = useState(first)
    return (
      <Modal kind="picker" open onClose={() => {}} title="Sample" historyEntry={false} step={step}>
        <button type="button" data-step={step} onClick={() => setStep(step === "long" ? "short" : "long")}>
          next
        </button>
      </Modal>
    )
  }

  it.each([
    ["long", "short", 600, 300],
    ["short", "long", 300, 600],
  ] as const)("keeps the %s step's height for the %s one until the window resizes", (first, _next, kept, after) => {
    render(<Steps first={first} />)
    fireEvent.click(screen.getByText("next"))
    // Held exactly, so a taller step scrolls inside it rather than growing it.
    expect(popup().offsetHeight).toBe(kept)
    at(COMPUTER - 100)
    expect(popup().offsetHeight).toBe(after)
  })
})

// A field a finger focuses in a cover can sit below what the soft keyboard
// leaves of the body.
describe("a field focused in a cover", () => {
  let resized: (() => void) | null = null
  beforeEach(() => {
    resized = null
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          resized = callback
        }
        observe() {}
        disconnect() {}
      },
    )
  })

  it("is brought into view when it takes focus, and again when the body shrinks", () => {
    at(PHONE)
    pointer("coarse")
    render(
      <Sample kind="form">
        <input aria-label="name" />
      </Sample>,
    )
    const field = screen.getByLabelText("name")
    const scroll = vi.fn()
    field.scrollIntoView = scroll
    act(() => field.focus())
    expect(scroll).toHaveBeenCalledTimes(1)
    act(() => resized?.())
    expect(scroll).toHaveBeenCalledTimes(2)
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

  // Half a phone's width is too narrow for a door's icon and label on one line.
  it("stacks a picker's doors one to a row on a phone", () => {
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
    expect(footer()!.className).toContain("grid-cols-1")
    expect(footer()!.querySelector('[data-slot="footer-divider"]')).toBeNull()
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

  // The second click of a double click lands on whatever the first put under
  // the pointer, so a destructive button counts only the first.
  it("counts only the first click of a double click on a destructive action", () => {
    const onSelect = vi.fn()
    render(
      <Modal
        kind="confirm"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        cancelLabel="Cancel"
        confirm={{ label: "Delete", tone: "destructive", onSelect }}
      />,
    )
    fireEvent.click(button("Delete"), { detail: 2 })
    expect(onSelect).not.toHaveBeenCalled()
    fireEvent.click(button("Delete"), { detail: 1 })
    expect(onSelect).toHaveBeenCalledTimes(1)
  })

  it("names a report's safe button as asked and sets a destructive action apart from it", () => {
    render(
      <Modal
        kind="report"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        closeLabel="Done"
        actions={[
          { label: "Refresh", tone: "plain", onSelect: () => {} },
          { label: "Stop everything…", tone: "destructive", onSelect: () => {} },
        ]}
      />,
    )
    const order = [...footer()!.querySelectorAll("button")].map((b) => b.textContent)
    expect(order).toEqual(["Stop everything…", "Refresh", "Done"])
    expect(cell("Stop everything…").className).toContain("mr-auto")
  })

  it("draws an action with an address as a link that opens in a new tab", () => {
    const onSelect = vi.fn()
    render(
      <Modal
        kind="report"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        actions={[{ label: "Open full notes", tone: "primary", href: "https://example.test/notes", onSelect }]}
      />,
    )
    const link = screen.getByRole("link", { name: "Open full notes" }) as HTMLAnchorElement
    expect(link.getAttribute("href")).toBe("https://example.test/notes")
    expect(link.target).toBe("_blank")
    expect(link.rel).toContain("noopener")
    link.addEventListener("click", (event) => event.preventDefault())
    fireEvent.click(link)
    expect(onSelect).toHaveBeenCalledTimes(1)
  })

  it("lays out any number of doors, and a toolbar outside the part that scrolls", () => {
    const door = (label: string) => ({ label, icon: FolderPlus, onSelect: () => {} })
    render(
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        toolbar={<button type="button">Sort</button>}
        doors={[door("One…"), door("Two…"), door("Three…")]}
      >
        <button type="button">row</button>
      </Modal>,
    )
    expect([...footer()!.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
      "One…",
      "Two…",
      "Three…",
    ])
    const body = document.querySelector('[data-slot="modal-body"]')!
    expect(body.contains(screen.getByText("row"))).toBe(true)
    expect(body.contains(screen.getByText("Sort"))).toBe(false)
    expect(popup().contains(screen.getByText("Sort"))).toBe(true)
  })

  it("hands the caller its Back", () => {
    const back = { current: null as HTMLButtonElement | null }
    render(
      <Modal
        kind="picker"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        back={{ label: "all things", onSelect: () => {} }}
        backRef={back}
      />,
    )
    expect(back.current).toBe(screen.getByRole("button", { name: "Back to all things" }))
  })

  it("hands the caller its safe button", () => {
    const safe = { current: null as HTMLButtonElement | null }
    render(
      <Modal
        kind="confirm"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        cancelLabel="Keep it"
        confirm={remove}
        safeRef={safe}
      />,
    )
    expect(safe.current).toBe(button("Keep it"))
  })

  // A disabled button drops keyboard focus to the page, so a busy one stays
  // focusable and says it is unavailable.
  it("keeps a busy action focusable", () => {
    render(
      <Modal
        kind="form"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        submit={{ label: "Save", tone: "primary", onSelect: () => {}, busy: true }}
      />,
    )
    expect((button("Save") as HTMLButtonElement).disabled).toBe(false)
    expect(button("Save").getAttribute("aria-disabled")).toBe("true")
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

describe("a card's frame", () => {
  it("is as wide and as tall as its tokens say, and hangs from the top when asked", () => {
    render(
      <Modal
        kind="report"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        size="w600"
        height="h640"
        anchor="top"
      />,
    )
    expect(popup().className).toContain("sm:max-w-[600px]")
    expect(popup().className).toContain("h-[min(40rem,calc(100dvh-2rem))]")
    expect(popup().className).toContain("translate-y-0")
  })
})

describe("where focus goes when a modal closes", () => {
  it("is where the caller says", async () => {
    function Closing({ open }: { open: boolean }) {
      const target = useRef<HTMLButtonElement | null>(null)
      return (
        <>
          <button type="button" ref={target}>
            elsewhere
          </button>
          <Modal kind="report" open={open} onClose={() => {}} title="Sample" historyEntry={false} finalFocus={target} />
        </>
      )
    }
    const view = render(<Closing open />)
    await waitFor(() => expect(popup().contains(document.activeElement)).toBe(true))
    view.rerender(<Closing open={false} />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByText("elsewhere")))
  })
})

// The page behind can take focus by itself, as a terminal that mounts again
// across a layout switch does.
describe("focus taken by the page behind an open modal", () => {
  // The page's control mounts after the modal opened, as the terminal does.
  function Page({ remounted }: { remounted: boolean }) {
    return (
      <>
        <div {...{ [PAGE_LAYER_ATTR]: "" }}>
          {remounted ? <button type="button">behind</button> : null}
        </div>
        <Sample kind="form">
          <input aria-label="first" />
          <input aria-label="second" />
        </Sample>
      </>
    )
  }

  it("comes back to where it was in the modal", async () => {
    const view = render(<Page remounted={false} />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("first")))
    act(() => screen.getByLabelText("second").focus())
    view.rerender(<Page remounted />)
    act(() => screen.getByText("behind").focus())
    expect(document.activeElement).toBe(screen.getByLabelText("second"))
  })

  // The offline cover, the sign-in gate and the toasts are drawn over the
  // modal, outside the page it covers.
  it("stays on a layer drawn over the modal", async () => {
    render(<Page remounted={false} />)
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("first")))
    const above = document.createElement("button")
    document.body.appendChild(above)
    act(() => above.focus())
    expect(document.activeElement).toBe(above)
    above.remove()
  })
})

// The editor overlay is a dialog itself, and its own dialogs open inside it.
describe("a modal opened inside another dialog", () => {
  function Inside({ onClose }: { onClose: (reason: string) => void }) {
    return (
      <Dialog open historyEntry={false}>
        <DialogContent>
          <DialogTitle>Editor</DialogTitle>
          <Modal
            kind="confirm"
            open
            onClose={onClose}
            title="Delete it?"
            historyEntry={false}
            cancelLabel="Cancel"
            confirm={remove}
          />
        </DialogContent>
      </Dialog>
    )
  }

  it("dims the dialog behind it and closes on a press outside it", async () => {
    const onClose = vi.fn()
    render(<Inside onClose={onClose} />)
    const confirm = await screen.findByRole("dialog", { name: "Delete it?" })
    const overlays = document.querySelectorAll('[data-slot="dialog-overlay"]')
    expect(overlays).toHaveLength(2)
    const own = overlays[1]
    expect(own.compareDocumentPosition(confirm) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    fireEvent.pointerDown(own, { button: 0 })
    fireEvent.mouseDown(own, { button: 0 })
    fireEvent.pointerUp(own, { button: 0 })
    fireEvent.mouseUp(own, { button: 0 })
    fireEvent.click(own, { button: 0 })
    expect(onClose).toHaveBeenCalledWith("outside")
  })
})

// Another device takes the row over while a confirm about it is open.
describe("a modal whose action changes a row", () => {
  it("disables the action with the reason while another device drives the row", async () => {
    render(
      <RowReasonContext.Provider value={useRowChangeReason}>
        <Modal
          kind="confirm"
          open
          onClose={() => {}}
          title="Close tab?"
          historyEntry={false}
          cancelLabel="Cancel"
          confirm={{ label: "Close tab", tone: "destructive", onSelect: () => {} }}
          row={{ sessionId: "s1" }}
        />
      </RowReasonContext.Provider>,
    )
    const action = () => screen.getByRole("button", { name: "Close tab" }) as HTMLButtonElement
    expect(action().disabled).toBe(false)
    let retire = () => {}
    act(() => {
      retire = registerPaneCover("pty-1", "card", {
        row: agentRowKey("s1"),
        driver: "other",
        device: "Firefox on Linux",
      })
    })
    expect(action().disabled).toBe(true)
    fireEvent.click(action().parentElement!)
    expect(
      (await screen.findAllByText(/^This agent is active on Firefox on Linux, so actions that modify it are disabled/))
        .length,
    ).toBeGreaterThan(0)
    act(() => retire())
    expect(action().disabled).toBe(false)
  })
})

describe("a form whose action changes a row", () => {
  it("runs no submit from Enter in a field while another device drives the row", () => {
    const submitted = vi.fn()
    render(
      <RowReasonContext.Provider value={useRowChangeReason}>
        <Modal
          kind="form"
          open
          onClose={() => {}}
          title="Rename agent"
          historyEntry={false}
          submit={save}
          row={{ sessionId: "s2" }}
        >
          <input
            aria-label="name"
            onKeyDown={(event) => {
              if (event.key === "Enter") submitted()
            }}
          />
        </Modal>
      </RowReasonContext.Provider>,
    )
    const field = screen.getByLabelText("name")
    fireEvent.keyDown(field, { key: "Enter" })
    expect(submitted).toHaveBeenCalledTimes(1)
    let retire = () => {}
    act(() => {
      retire = registerPaneCover("pty-2", "card", {
        row: agentRowKey("s2"),
        driver: "other",
        device: "Firefox on Linux",
      })
    })
    fireEvent.keyDown(field, { key: "Enter" })
    expect(submitted).toHaveBeenCalledTimes(1)
    act(() => retire())
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

describe("the way out base-ui reports", () => {
  it("is told to the caller as the way out it was", () => {
    expect(closeReasonFrom("escape-key")).toBe("escape")
    expect(closeReasonFrom("outside-press")).toBe("outside")
    expect(closeReasonFrom("focus-out")).toBe("outside")
    expect(closeReasonFrom("close-press")).toBe("close-control")
    expect(closeReasonFrom("imperative-action")).toBeNull()
    expect(closeReasonFrom("none")).toBeNull()
  })
})

describe("why a modal closed", () => {
  it.each([
    ["cancel", () => fireEvent.click(screen.getByRole("button", { name: "Cancel" }))],
    ["escape", () => fireEvent.keyDown(popup(), { key: "Escape" })],
  ] as const)("is told to the caller: %s", async (reason, leave) => {
    const onClose = vi.fn()
    render(<Sample kind="form" onClose={onClose} />)
    await waitFor(() => expect(popup().contains(document.activeElement)).toBe(true))
    leave()
    expect(onClose).toHaveBeenCalledWith(reason)
  })

  // Preferences: the first Escape clears its search, the second closes.
  it("lets the caller answer a way out itself and stay open", async () => {
    const onClose = vi.fn()
    let searching = true
    render(
      <Modal
        kind="form"
        open
        onClose={onClose}
        title="Sample"
        historyEntry={false}
        submit={save}
        beforeClose={(reason) => {
          if (reason !== "escape" || !searching) return "close"
          searching = false
          return "stay"
        }}
      />,
    )
    await waitFor(() => expect(popup().contains(document.activeElement)).toBe(true))
    fireEvent.keyDown(popup(), { key: "Escape" })
    expect(onClose).not.toHaveBeenCalled()
    fireEvent.keyDown(popup(), { key: "Escape" })
    expect(onClose).toHaveBeenCalledWith("escape")
  })

  it("closes at once when its target vanishes, busy and unsaved or not", () => {
    const onClose = vi.fn()
    const form = (vanished: boolean) => (
      <Modal
        kind="form"
        open
        busy
        vanished={vanished}
        onClose={onClose}
        title="Sample"
        historyEntry={false}
        submit={save}
        unsavedEdits={{ holder: "The sample has", changes: 2, leaves: ["the sample"] }}
      />
    )
    const view = render(form(false))
    expect(onClose).not.toHaveBeenCalled()
    view.rerender(form(true))
    expect(onClose).toHaveBeenCalledWith("vanished")
    expect(screen.queryByText("Discard unsaved changes?")).toBeNull()
  })
})

describe("a form with unsaved edits", () => {
  function Editing({
    changes,
    onClose,
    open = true,
    setAside,
  }: {
    changes: number
    onClose: () => void
    open?: boolean
    setAside?: () => void
  }) {
    return (
      <Modal
        kind="form"
        open={open}
        onClose={onClose}
        title="Sample"
        submit={save}
        unsavedEdits={{
          holder: "The sample has",
          changes,
          leaves: ["the file ", chip("notes.md")],
          setAside,
        }}
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
    expect(onClose).toHaveBeenCalledWith("escape")
  })

  it("gives focus back to the field being typed in when the question is answered Keep editing", async () => {
    render(
      <Modal
        kind="form"
        open
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        submit={save}
        unsavedEdits={{ holder: "The sample has", changes: 2, leaves: ["the sample"] }}
      >
        <input aria-label="first" />
        <textarea aria-label="notes" />
      </Modal>,
    )
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("first")))
    act(() => screen.getByLabelText("notes").focus())
    const cancel = screen.getByRole("button", { name: "Cancel" })
    act(() => cancel.focus())
    fireEvent.click(cancel)
    const keep = await screen.findByRole("button", { name: "Keep editing" })
    await waitFor(() => expect(document.activeElement).toBe(keep))
    fireEvent.click(keep)
    await waitFor(() => expect(asking()).toBeNull())
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("notes")))
  })

  it("says what discarding leaves as it was, in the caller's words", async () => {
    render(<Editing changes={2} onClose={() => {}} />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    const sentence = (await screen.findByText(/Discarding them leaves/)).textContent
    expect(sentence).toBe(
      "The sample has 2 unsaved changes. Discarding them leaves the file notes.md as it was.",
    )
    expect(screen.getByText("notes.md", { selector: "code" })).toBeTruthy()
  })

  it("asks as a confirm, its two buttons sharing one row on a phone", async () => {
    at(PHONE)
    render(<Editing changes={2} onClose={() => {}} />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    const keep = await screen.findByRole("button", { name: "Keep editing" })
    const question = keep.closest<HTMLElement>('[data-slot="dialog-content"]')!
    expect(question.dataset.kind).toBe("confirm")
    expect(question.querySelector(".grid-cols-2")).not.toBeNull()
    await waitFor(() => expect(document.activeElement).toBe(keep))
  })

  it("closes at once when nothing was changed", () => {
    const onClose = vi.fn()
    render(<Editing changes={0} onClose={onClose} />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(onClose).toHaveBeenCalledTimes(1)
    expect(asking()).toBeNull()
  })

  it("does not ask again on the next open after it was closed from outside", async () => {
    const view = render(<Editing changes={2} onClose={() => {}} />)
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    await waitFor(() => expect(asking()).not.toBeNull())
    view.rerender(<Editing changes={2} onClose={() => {}} open={false} />)
    await waitFor(() => expect(asking()).toBeNull())
    view.rerender(<Editing changes={2} onClose={() => {}} />)
    await waitFor(() => expect(popup()).not.toBeNull())
    expect(asking()).toBeNull()
  })
})

describe("a busy modal", () => {
  function Saving({ busy, onClose, kind = "form" }: { busy: boolean; onClose: () => void; kind?: "form" | "picker" }) {
    if (kind === "picker") {
      return <Modal kind="picker" open busy={busy} onClose={onClose} title="Sample" historyEntry={false} />
    }
    return <Modal kind="form" open busy={busy} onClose={onClose} title="Sample" submit={save} />
  }
  const cancel = () => screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement

  it("refuses every way out, which a modal that is not busy takes", async () => {
    const onClose = vi.fn()
    const view = render(<Saving busy onClose={onClose} />)
    await waitFor(() => expect(popup().contains(document.activeElement)).toBe(true))
    fireEvent.keyDown(document.activeElement!, { key: "Escape" })
    fireEvent.click(cancel())
    expect(onClose).not.toHaveBeenCalled()
    view.rerender(<Saving busy={false} onClose={onClose} />)
    fireEvent.keyDown(document.activeElement!, { key: "Escape" })
    expect(onClose).toHaveBeenCalledWith("escape")
  })

  it("takes no press on its footer actions or its step's Back, and keeps them focusable", () => {
    const pressed = vi.fn()
    render(
      <Modal
        kind="form"
        open
        busy
        onClose={() => {}}
        title="Sample"
        historyEntry={false}
        back={{ label: "Start", onSelect: pressed }}
        secondary={{ label: "Other", tone: "plain", onSelect: pressed }}
        submit={{ label: "Save", tone: "primary", onSelect: pressed }}
      />,
    )
    for (const name of ["Back to Start", "Other", "Save"]) {
      const control = screen.getByRole("button", { name }) as HTMLButtonElement
      fireEvent.click(control)
      expect(control.disabled, name).toBe(false)
      expect(control.getAttribute("aria-disabled"), name).toBe("true")
    }
    expect(pressed).not.toHaveBeenCalled()
  })

  it.each([
    ["the modal", { busy: true, submit: save }],
    ["its submit", { submit: { ...save, busy: true } }],
  ] as const)("runs no submit from Enter in a field while %s is busy", (_what, props) => {
    const submitted = vi.fn()
    render(
      <Modal kind="form" open onClose={() => {}} title="Sample" historyEntry={false} {...props}>
        <input
          aria-label="name"
          onKeyDown={(event) => {
            if (event.key === "Enter") submitted()
          }}
        />
      </Modal>,
    )
    fireEvent.keyDown(screen.getByLabelText("name"), { key: "Enter" })
    expect(submitted).not.toHaveBeenCalled()
  })

  // A disabled button would drop the focus it holds to the page.
  it("shows and announces that its ways out do nothing, and keeps them focusable", () => {
    for (const kind of ["form", "picker"] as const) {
      for (const width of [COMPUTER, PHONE]) {
        at(width)
        render(<Saving busy kind={kind} onClose={() => {}} />)
        const ways = kind === "form" && width === COMPUTER ? [cancel()] : closeButtons()
        expect(ways.length, `${kind} at ${width}`).toBeGreaterThan(0)
        for (const way of ways) {
          expect(way.getAttribute("aria-disabled"), `${kind} at ${width}`).toBe("true")
          expect((way as HTMLButtonElement).disabled).toBe(false)
        }
        cleanup()
      }
    }
  })
})

describe("an IME composition in a field", () => {
  it.each([
    ["isComposing", false, { isComposing: true }],
    ["keyCode 229", false, { keyCode: 229 }],
    ["isComposing", true, { isComposing: true }],
    ["keyCode 229", true, { keyCode: 229 }],
  ] as const)(
    "keeps an Enter that ends it (%s) from the caller's handler and leaves it to the field (busy: %s)",
    (_how, busy, init) => {
      const pressed = vi.fn()
      render(
        <Modal kind="form" open onClose={() => {}} title="Sample" historyEntry={false} busy={busy} submit={save}>
          <input
            aria-label="name"
            onKeyDown={(event) => {
              if (event.key === "Enter") pressed()
            }}
          />
        </Modal>,
      )
      const notPrevented = fireEvent.keyDown(screen.getByLabelText("name"), { key: "Enter", ...init })
      expect(pressed).not.toHaveBeenCalled()
      expect(notPrevented).toBe(true)
      fireEvent.keyDown(screen.getByLabelText("name"), { key: "Enter" })
      expect(pressed).toHaveBeenCalledTimes(busy ? 0 : 1)
    },
  )
})

// The component that holds a modal's state and reads the store for it exists
// from the open until the close has finished, and not while closed.
describe("a modal's lifetime", () => {
  let mounts = 0
  function Inner({ open, onClosed }: { open: boolean; onClosed: () => void }) {
    useState(() => (mounts += 1))
    return <Modal kind="report" open={open} onClose={() => {}} onClosed={onClosed} title="Sample" historyEntry={false} />
  }
  function Host({ open }: { open: boolean }) {
    const life = useModalLifetime(open)
    return life.mounted ? <Inner key={life.epoch} open={open} onClosed={life.onClosed} /> : null
  }

  it("mounts on open, lasts through the close and starts fresh on the next open", async () => {
    mounts = 0
    const view = render(<Host open={false} />)
    expect(mounts).toBe(0)
    view.rerender(<Host open />)
    expect(mounts).toBe(1)
    view.rerender(<Host open={false} />)
    await waitFor(() => expect(document.querySelector('[data-slot="dialog-content"]')).toBeNull())
    view.rerender(<Host open />)
    expect(mounts).toBe(2)
  })
})

describeModalBack(browser)
