// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

// Every store action an entry can reach, as spies: the corner must run the
// shared creation lists' own actions, never a copy of them.
const { calls, spy, state } = vi.hoisted(() => {
  const calls: string[] = []
  const spy =
    (name: string) =>
    (...args: unknown[]) =>
      void calls.push(
        args.length ? `${name}(${args.map(String).join(", ")})` : name,
      )
  return { calls, spy, state: { current: {} as Record<string, unknown> } }
})
vi.mock("@/lib/store", () => ({
  useDuxSelector: (select: (s: unknown) => unknown) => select(state.current),
  openAgentTerminalPicker: spy("openAgentTerminalPicker"),
  createStandaloneTerminal: spy("createStandaloneTerminal"),
  openNewAgentPicker: spy("openNewAgentPicker"),
  openCreateAgentFromPr: spy("openCreateAgentFromPr"),
  openStandaloneAgentPicker: spy("openStandaloneAgentPicker"),
  openAddProject: spy("openAddProject"),
  openAddProjectForInit: spy("openAddProjectForInit"),
  openCloneProject: spy("openCloneProject"),
}))

import { LauncherCorner } from "@/components/LauncherCorner"

function seed({ projects = 1, sessions = 1 } = {}) {
  state.current = {
    bootstrap: { gh_available: true },
    spine: {
      projects: Array.from({ length: projects }, (_, i) => ({ id: `p${i}` })),
      sessions: Array.from({ length: sessions }, (_, i) => ({ id: `s${i}` })),
    },
  }
}

const settle = () => new Promise((r) => setTimeout(r, 40))

async function openMenu() {
  fireEvent.click(screen.getByRole("button", { name: "More ways to start" }))
  await screen.findByRole("menu")
  await settle()
}

// A menu entry by the title it shows above its sentence.
const entry = (title: string) =>
  screen
    .getAllByRole("menuitem")
    .find((item) => item.querySelector("[data-entry-title]")?.textContent === title)

// What a holder's own `[&...]:utility` arbitrary variants apply to an element,
// as the browser would read them: each variant becomes its selector, rooted at
// the holder, and applies wherever that selector matches.
function appliedVariants(holder: Element, element: Element) {
  const applied: string[] = []
  for (const token of holder.className.split(/\s+/)) {
    const rule = /^\[(&.+)\]:([\w-]+)$/.exec(token)
    if (!rule) continue
    const selector = rule[1].replace(/&/g, ":scope").replace(/_/g, " ")
    if (Array.from(holder.querySelectorAll(selector)).includes(element)) {
      applied.push(rule[2])
    }
  }
  return applied
}

const squaredCorners = (element: Element) =>
  appliedVariants(element.closest('[data-slot="button-group"]')!, element).filter(
    (utility) => /^rounded-[lr]-none$/.test(utility),
  )

beforeEach(() => {
  calls.length = 0
  seed()
})
afterEach(() => cleanup())

describe("LauncherCorner", () => {
  it("opens the New agent project picker from its main half", () => {
    render(<LauncherCorner />)
    fireEvent.click(screen.getByRole("button", { name: "New agent…" }))
    expect(calls).toEqual(["openNewAgentPicker(new)"])
  })

  it("names its menu half and says it opens a menu", () => {
    render(<LauncherCorner />)
    const more = screen.getByRole("button", { name: "More ways to start" })
    expect(more.getAttribute("aria-haspopup")).toBe("menu")
  })

  it.each([
    ["New agent from PR…", "Start an agent on a GitHub pull request's branch.", "openCreateAgentFromPr(null)"],
    ["New agent from existing worktree…", "Use a copy of a project already on disk, on its branch.", "openNewAgentPicker(from_worktree)"],
    ["New standalone agent…", "Run an agent in any folder, no project or git needed.", "openStandaloneAgentPicker"],
    ["New terminal for an agent…", "Open a command line in an agent's copy of the code.", "openAgentTerminalPicker"],
    ["New standalone terminal", "Opens right away, in your home folder.", "createStandaloneTerminal"],
    ["Add project…", "Point dux at a git repository on this computer.", "openAddProject"],
    ["Clone a repository…", "Download a repository, then add it as a project.", "openCloneProject"],
    ["Initialize a repository…", "Turn a folder into a new git repository and project.", "openAddProjectForInit"],
  ])("lists %s with its sentence and runs the shared action", async (title, sentence, action) => {
    render(<LauncherCorner />)
    await openMenu()
    const item = entry(title)!
    expect(item.textContent).toBe(title + sentence)
    fireEvent.click(item)
    expect(calls).toEqual([action])
  })

  it("groups the entries under Agents, Terminals and Projects, without the main half's", async () => {
    render(<LauncherCorner />)
    await openMenu()
    expect(
      Array.from(document.querySelectorAll('[data-slot="dropdown-menu-label"]')).map(
        (label) => label.textContent,
      ),
    ).toEqual(["Agents", "Terminals", "Projects"])
    expect(entry("New agent…")).toBeUndefined()
    expect(screen.getAllByRole("menuitem")).toHaveLength(8)
  })

  it("disables what needs a project on a confirmed-empty workspace, as the dialog does", async () => {
    seed({ projects: 0, sessions: 0 })
    render(<LauncherCorner />)
    await openMenu()
    const disabled = (title: string) =>
      entry(title)!.getAttribute("aria-disabled") === "true"
    expect(disabled("New agent from PR…")).toBe(true)
    expect(disabled("New agent from existing worktree…")).toBe(true)
    expect(disabled("New terminal for an agent…")).toBe(true)
    expect(disabled("New standalone agent…")).toBe(false)
    expect(disabled("New standalone terminal")).toBe(false)
    expect(disabled("Add project…")).toBe(false)
    expect(
      screen.getByText("Add a project first, then start agents on it."),
    ).toBeTruthy()
  })

  it("leaves the from-PR entry out where gh is not available", async () => {
    state.current = { ...state.current, bootstrap: { gh_available: false } }
    render(<LauncherCorner />)
    await openMenu()
    expect(entry("New agent from PR…")).toBeUndefined()
  })
  // The sidebar footer and the phone hub's bottom bar, whose menu is a sheet.
  describe.each([
    ["a computer", 1024],
    ["a phone", 390],
  ])("on %s", (_, width) => {
    const desktopWidth = window.innerWidth
    beforeEach(() => {
      Object.defineProperty(window, "innerWidth", { configurable: true, value: width })
    })
    afterEach(() => {
      Object.defineProperty(window, "innerWidth", { configurable: true, value: desktopWidth })
    })

    it("keeps the menu half's outer corners round, with its menu closed and open", async () => {
      render(<LauncherCorner />)
      const more = screen.getByRole("button", { name: "More ways to start" })
      const main = screen.getByRole("button", { name: "New agent…" })
      expect(squaredCorners(more)).toEqual(["rounded-l-none"])
      expect(squaredCorners(main)).toEqual(["rounded-r-none"])
      await openMenu()
      expect(squaredCorners(more)).toEqual(["rounded-l-none"])
      expect(squaredCorners(main)).toEqual(["rounded-r-none"])
    })

    it("points its menu half's chevron down while closed and up while open", async () => {
      render(<LauncherCorner />)
      const more = screen.getByRole("button", { name: "More ways to start" })
      const chevron = more.querySelector("svg")!
      const rotated = () => appliedVariants(more, chevron).includes("rotate-180")
      expect(rotated()).toBe(false)
      await openMenu()
      expect(rotated()).toBe(true)
    })
  })
})
