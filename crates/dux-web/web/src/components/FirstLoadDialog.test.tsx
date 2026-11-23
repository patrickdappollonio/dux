// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"

import type { Bootstrap } from "@/lib/bootstrapApi"
import { NO_NOTES_EXPLANATION } from "@/lib/releaseNotes"
import { installBrowserHistory } from "@/test/browserHistory"
import type { DuxState, FirstLoadDialogState } from "@/lib/store"

// The ONE dialog serves both screens, so what matters here is the SELECTION:
// which content and which two buttons each screen gets, and that the primary
// action of each is wired to the right thing.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeFirstLoad: vi.fn(),
    openAddProject: vi.fn(),
  }
})

function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()
// A history the store can give a dialog an entry in, so Back reaches it the
// way the browser delivers it.
const browser = installBrowserHistory()

const { FirstLoadDialog } = await import("./FirstLoadDialog")
const store = await import("@/lib/store")
const closeFirstLoad = vi.mocked(store.closeFirstLoad)
const openAddProject = vi.mocked(store.openAddProject)

const NOTES = {
  version: "v0.6.0",
  headline: "Quieter plumbing, louder failures",
  paragraphs: ["Version 0.6.0 is a tune-up release."],
  sections: [
    "Environment config for agents and terminals",
    "A website exists now",
  ],
  html_url: "https://github.com/patrickdappollonio/dux/releases/tag/v0.6.0",
}

const bootstrap = {
  website_url: "https://getdux.app",
  welcome_screen: {
    tagline: "One git worktree per coding agent, and a real terminal.",
    paragraphs: [
      "Start by adding a project: any git repo on this machine.",
      "Your config file lives at /home/ada/.config/dux/config.toml.",
    ],
    steps: [
      {
        number: 1,
        title: "Add a project",
        detail: "Point dux at any git repo.",
      },
      {
        number: 2,
        title: "Create an agent",
        detail: "It gets its own worktree.",
      },
      {
        number: 3,
        title: "Launch",
        detail: "Your provider CLI runs in a terminal.",
      },
    ],
  },
} as unknown as Bootstrap

function seed(firstLoad: Partial<FirstLoadDialogState> | null) {
  mockState = {
    firstLoad:
      firstLoad === null
        ? null
        : ({
            screen: "welcome",
            automatic: true,
            notes: null,
            loading: false,
            error: null,
            ...firstLoad,
          } as FirstLoadDialogState),
    bootstrap,
  } as unknown as DuxState
}

/** The dialog footer, so a query for "Close" cannot also match the dialog's own
 *  sr-only X close button in the corner. */
function footer(): HTMLElement {
  const el = document.querySelector('[data-slot="modal-footer"]')
  if (el === null) throw new Error("no dialog footer rendered")
  return el as HTMLElement
}

/** The footer's buttons and links, by name, in the order they are drawn. */
function footerControls(): string[] {
  return [...footer().querySelectorAll("button, a")].map((el) => el.textContent ?? "")
}

/** The numbered-steps list, so a query for "Add a project" as a STEP TITLE
 *  cannot also match the primary button of the same name. */
function steps(): HTMLElement {
  const el = document.querySelector("ol")
  if (el === null) throw new Error("no steps list rendered")
  return el as HTMLElement
}

beforeEach(() => {
  installBootStubs()
  closeFirstLoad.mockClear()
  openAddProject.mockClear()
})

afterEach(() => cleanup())

describe("FirstLoadDialog", () => {
  it("renders nothing when no screen is open", () => {
    seed(null)
    render(<FirstLoadDialog />)
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  // Closing a screen that opened by itself records it as seen, so the
  // browser's Back must never be what closes it: it gets no history entry, and
  // Back leaves it alone. Opened from the menu, it is an ordinary dialog.
  it("takes no history entry when it opened by itself, and Back never acknowledges it", async () => {
    seed({ automatic: true })
    const before = browser.index
    render(<FirstLoadDialog />)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(browser.index).toBe(before)
    act(() => browser.back())
    expect(closeFirstLoad).not.toHaveBeenCalled()
  })

  it("closes on Back when it was opened from the menu", async () => {
    seed({ automatic: false })
    const before = browser.index
    render(<FirstLoadDialog />)
    await waitFor(() => {
      expect(browser.index).toBe(before + 1)
    })
    act(() => browser.back())
    expect(closeFirstLoad).toHaveBeenCalledOnce()
  })

  it("stands down on the standalone editor surface", () => {
    // The onboarding belongs to the main shell: a tab that is nothing but
    // the editor must not open the welcome/what's-new dialog over it.
    seed({})
    mockState = { ...mockState, standaloneEditor: true } as DuxState
    render(<FirstLoadDialog />)
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  describe("the welcome screen", () => {
    it("shows the title, the tagline, the prose, and the three numbered steps", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      expect(screen.getByText("Welcome to dux")).toBeTruthy()
      expect(
        screen.getByText(
          "One git worktree per coding agent, and a real terminal.",
        ),
      ).toBeTruthy()
      // The prose, including the machine's own config path.
      expect(
        screen.getByText(/\/home\/ada\/\.config\/dux\/config\.toml/),
      ).toBeTruthy()
      // The numbered steps DELIBERATELY repeat the prose, so a skimmer can act.
      for (const [n, title] of [
        ["1", "Add a project"],
        ["2", "Create an agent"],
        ["3", "Launch"],
      ]) {
        expect(within(steps()).getByText(n)).toBeTruthy()
        expect(within(steps()).getByText(title)).toBeTruthy()
      }
    })

    it("offers exactly the two approved buttons, with Add a project as the filled primary", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      const add = within(footer()).getByRole("button", {
        name: /add a project/i,
      })
      const site = within(footer()).getByRole("link", {
        name: /visit the website/i,
      })
      // The primary is the filled one; the link button is outlined.
      expect(add.className).toContain("bg-primary")
      expect(site.className).toContain("border-border")
      // And nothing else: two buttons in the footer, the website first, and
      // the way out is the corner close, per the approved design.
      expect(footerControls()).toEqual(["Visit the website", "Add a project"])
      const close = screen.getByRole("button", { name: "Close" })
      expect(footer().contains(close)).toBe(false)
    })

    it("closes the dialog and opens the project picker from the primary action", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      fireEvent.click(
        within(footer()).getByRole("button", { name: /add a project/i }),
      )
      // Closing an AUTOMATIC screen is what dismisses it (the store posts that).
      expect(closeFirstLoad).toHaveBeenCalledOnce()
      expect(openAddProject).toHaveBeenCalledOnce()
    })

    it("links the secondary button to the server-projected website and shows the destination", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      const site = within(footer()).getByRole("link", {
        name: /visit the website/i,
      })
      expect(site.getAttribute("href")).toBe("https://getdux.app")
      expect(site.getAttribute("target")).toBe("_blank")
      expect(site.getAttribute("rel")).toContain("noopener")
      // The footer names the destination before it is clicked.
      expect(screen.getByText("https://getdux.app")).toBeTruthy()
    })

    it("dismisses the screen when the website link is used, so the version is recorded", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      fireEvent.click(
        within(footer()).getByRole("link", { name: /visit the website/i }),
      )
      // The TUI dismisses and THEN opens the URL. Without this, a user who
      // follows the link and closes the tab records nothing and sees the same
      // screen next launch.
      expect(closeFirstLoad).toHaveBeenCalledOnce()
    })

    it("says so rather than rendering an empty frame when an older server sends no copy", () => {
      seed({ screen: "welcome" })
      mockState = { ...mockState, bootstrap: {} as Bootstrap }
      render(<FirstLoadDialog />)
      expect(
        screen.getByText("This server did not send the welcome text."),
      ).toBeTruthy()
    })
  })

  describe("the what's-new screen", () => {
    it("shows the version chip, the headline, the intro, and the feature titles", () => {
      seed({ screen: "whats_new", notes: NOTES })
      render(<FirstLoadDialog />)

      expect(screen.getByText("What's new in")).toBeTruthy()
      expect(screen.getByText("v0.6.0")).toBeTruthy()
      expect(screen.getByText("Quieter plumbing, louder failures")).toBeTruthy()
      expect(
        screen.getByText("Version 0.6.0 is a tune-up release."),
      ).toBeTruthy()
      expect(screen.getByText("In this release")).toBeTruthy()
      for (const section of NOTES.sections) {
        expect(screen.getByText(section)).toBeTruthy()
      }
    })

    it("offers exactly the two approved buttons, with Open full notes as the primary", () => {
      seed({ screen: "whats_new", notes: NOTES })
      render(<FirstLoadDialog />)

      const open = within(footer()).getByRole("link", {
        name: /open full notes/i,
      })
      expect(open.getAttribute("href")).toBe(NOTES.html_url)
      expect(open.className).toContain("bg-primary")
      // Close first and the primary last, as before the dialog moved, with
      // the corner close beside them.
      expect(footerControls()).toEqual(["Close", "Open full notes"])
      expect(screen.getAllByRole("button", { name: "Close" })).toHaveLength(2)
      // The welcome screen's buttons must not leak in.
      expect(within(footer()).queryByText(/add a project/i)).toBeNull()
      // The footer names where the primary goes.
      expect(screen.getByText(NOTES.html_url)).toBeTruthy()
    })

    it("closes from the Close button", () => {
      seed({ screen: "whats_new", notes: NOTES })
      render(<FirstLoadDialog />)
      fireEvent.click(
        within(footer()).getByRole("button", { name: /^close$/i }),
      )
      expect(closeFirstLoad).toHaveBeenCalledOnce()
    })

    it("dismisses the screen when the full-notes link is used, matching the TUI", () => {
      seed({ screen: "whats_new", notes: NOTES })
      render(<FirstLoadDialog />)

      const open = within(footer()).getByRole("link", {
        name: /open full notes/i,
      })
      fireEvent.click(open)
      expect(closeFirstLoad).toHaveBeenCalledOnce()
      // And it is still a real link: the handler must not have replaced the
      // navigation, only preceded it.
      expect(open.getAttribute("href")).toBe(NOTES.html_url)
    })

    it("shows a real loading state and keeps the notes link genuinely inert until they arrive", () => {
      seed({ screen: "whats_new", loading: true, automatic: false })
      render(<FirstLoadDialog />)

      expect(
        screen.getByText("Fetching the release notes from GitHub…"),
      ).toBeTruthy()
      // MEASURED fact behind this assertion: `<Button disabled render={<a
      // href=…/>} />` renders an anchor that KEEPS its href, and the CSS
      // `:disabled` pseudo-class does not match `<a>`, so a "disabled" link
      // still navigates. The component therefore renders a real <button> until
      // the URL is in hand. Assert the element type, not just an attribute.
      const open = within(footer()).getByRole("button", {
        name: /open full notes/i,
      })
      expect(open.tagName).toBe("BUTTON")
      expect(open.hasAttribute("href")).toBe(false)
      expect(open.hasAttribute("disabled")).toBe(true)
      expect(within(footer()).queryByRole("link")).toBeNull()
    })

    // A body that renders `notes.paragraphs` and `notes.sections` and nothing
    // else turns a headline-only release into a dialog with a title, two
    // buttons, and an entirely blank middle. That shape
    // is reachable without anyone doing anything unusual: GitHub prepends
    // `## What's Changed` and the release workflow appends `## Installation`, so a
    // one-line human headline is all the server-side parser is left with.
    it("explains itself when the release body had nothing the parser could read", () => {
      seed({
        screen: "whats_new",
        notes: {
          ...NOTES,
          headline: "Quieter plumbing, louder failures",
          paragraphs: [],
          sections: [],
        },
      })
      render(<FirstLoadDialog />)

      // The headline is still the title.
      expect(screen.getByText("Quieter plumbing, louder failures")).toBeTruthy()
      expect(screen.getByText(NO_NOTES_EXPLANATION)).toBeTruthy()
      // No label over an empty list.
      expect(screen.queryByText("In this release")).toBeNull()
      expect(screen.queryByRole("list")).toBeNull()
      // The escape hatch the copy points at is really there.
      expect(
        within(footer()).getByRole("link", { name: /open full notes/i }),
      ).toBeTruthy()
    })

    it("explains itself when the only feature title collapsed to nothing", () => {
      // A `### **__**` heading strips to "", which must not render the "In
      // this release" label above one blank bullet.
      seed({
        screen: "whats_new",
        notes: { ...NOTES, paragraphs: [], sections: [""] },
      })
      render(<FirstLoadDialog />)
      expect(screen.getByText(NO_NOTES_EXPLANATION)).toBeTruthy()
      expect(screen.queryByText("In this release")).toBeNull()
    })

    it("never shows the no-notes explanation when there are real notes", () => {
      seed({ screen: "whats_new", notes: NOTES })
      render(<FirstLoadDialog />)
      expect(screen.queryByText(NO_NOTES_EXPLANATION)).toBeNull()
    })

    it("shows a failed fetch in the body, not just as a vanished toast", () => {
      seed({
        screen: "whats_new",
        automatic: false,
        error: "GitHub returned HTTP 503",
      })
      render(<FirstLoadDialog />)
      expect(screen.getByText("GitHub returned HTTP 503")).toBeTruthy()
      expect(
        screen.queryByText("Fetching the release notes from GitHub…"),
      ).toBeNull()
    })
  })

  // One renderer covers desktop AND mobile (GlobalOverlays mounts it once for
  // both shells): a report, which the Modal component draws as a card on a
  // computer and as a cover on a phone.
  it("is a report 700px wide, comfortably wider than a routine dialog", () => {
    seed({ screen: "welcome" })
    render(<FirstLoadDialog />)
    const popup = document.querySelector('[data-slot="dialog-content"]')!
    expect(popup.getAttribute("data-kind")).toBe("report")
    expect(popup.className).toContain("700px")
  })

  // The split is driven by useIsMobile, whose snapshot reads window.innerWidth,
  // so these tests resize jsdom's window. The original width is captured and
  // restored: hardcoding a "desktop" width on the way out would quietly
  // redefine the default every other test in the file runs at.
  describe("on a phone", () => {
    const ORIGINAL_WIDTH = window.innerWidth

    function setViewportWidth(width: number) {
      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        writable: true,
        value: width,
      })
    }

    const titleRow = () => document.querySelector<HTMLElement>('[data-slot="modal-header"]')!
    const body = () => document.querySelector<HTMLElement>('[data-slot="modal-body"]')!

    beforeEach(() => setViewportWidth(390))
    afterEach(() => setViewportWidth(ORIGINAL_WIDTH))

    it("covers the screen, with the dux mark and the title in the title row", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      expect(
        document.querySelector('[data-slot="dialog-content"]')!.getAttribute("data-presentation"),
      ).toBe("cover")
      // The real logo mark, decorative to screen readers, so the title's name
      // is its words alone.
      const mark = titleRow().querySelector('img[src="/dux-logo.png"]')!
      expect(mark.getAttribute("aria-hidden")).toBe("true")
      expect(mark.getAttribute("alt")).toBe("")
      expect(within(titleRow()).getByText("Welcome to dux")).toBeTruthy()
      // Exactly one logo: no duck column beside the prose.
      expect(document.querySelectorAll('img[src="/dux-logo.png"]')).toHaveLength(1)
    })

    it("scrolls the tagline with the prose it introduces, under the pinned title row", () => {
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      expect(within(body()).getByText(/Point dux at any git repo\./)).toBeTruthy()
      expect(
        within(body()).getByText("One git worktree per coding agent, and a real terminal."),
      ).toBeTruthy()
      expect(body().contains(titleRow())).toBe(false)
      expect(body().contains(footer())).toBe(false)
    })

    it("shows the version chip with the what's-new notes", () => {
      seed({ screen: "whats_new", notes: NOTES })
      render(<FirstLoadDialog />)

      expect(within(titleRow()).getByText("Quieter plumbing, louder failures")).toBeTruthy()
      expect(within(body()).getByText("What's new in")).toBeTruthy()
      expect(within(body()).getByText("v0.6.0")).toBeTruthy()
      expect(within(body()).getByText("In this release")).toBeTruthy()
    })

    it("shows the duck's column and no mark in the title at desktop width", () => {
      setViewportWidth(1280)
      seed({ screen: "welcome" })
      render(<FirstLoadDialog />)

      const logos = document.querySelectorAll('img[src="/dux-logo.png"]')
      expect(logos).toHaveLength(1)
      expect(titleRow().contains(logos[0])).toBe(false)
      // The hairline divider between the art column and the prose.
      expect(logos[0].parentElement!.className).toContain("border-r")
      expect(logos[0].getAttribute("aria-hidden")).toBe("true")
    })
  })
})
