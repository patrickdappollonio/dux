// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

// Every store action a row can reach, as spies: the rows must run the shared
// creation lists' own actions, never a copy of them.
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
  useDux: () => state.current,
  closeNewLauncher: spy("closeNewLauncher"),
  createTerminal: spy("createTerminal"),
  createStandaloneTerminal: spy("createStandaloneTerminal"),
  openNewAgentPicker: spy("openNewAgentPicker"),
  openCreateAgentFromPr: spy("openCreateAgentFromPr"),
  openStandaloneAgentPicker: spy("openStandaloneAgentPicker"),
  openAddProject: spy("openAddProject"),
  openAddProjectForInit: spy("openAddProjectForInit"),
  openCloneProject: spy("openCloneProject"),
}))

import { NewLauncherDialog } from "@/components/NewLauncherDialog"

const managed = (id: string, title: string, project: string) => ({
  id,
  title,
  status: "active",
  working: false,
  workspace: {
    kind: "managed",
    project_id: project,
    branch_name: title,
    worktree_path: `/home/me/wt/${title}`,
    worktree_label: `~/wt/${title}`,
  },
})

function seed({
  projects = [{ id: "p1", name: "demo-api" }],
  sessions = [managed("s1", "polish-onboarding", "p1")] as unknown[],
} = {}) {
  state.current = {
    newLauncherOpen: true,
    bootstrap: { gh_available: true },
    spine: {
      projects,
      sessions,
      sidebar: {
        groups: projects.map((p) => ({
          project_id: p.id,
          name: p.name,
          orphaned: false,
          path_missing: false,
          session_ids: [],
        })),
        agentless_start: null,
      },
    },
  }
}

const row = (title: string) =>
  screen
    .getAllByRole("button")
    .find((b) => b.querySelector("span span")?.textContent === title)!

beforeEach(() => {
  calls.length = 0
  seed()
})
afterEach(() => cleanup())

describe("NewLauncherDialog", () => {
  it.each([
    ["New agent", "openNewAgentPicker(new)"],
    ["New agent from PR", "openCreateAgentFromPr(null)"],
    ["New agent from existing worktree", "openNewAgentPicker(from_worktree)"],
    ["New standalone agent", "openStandaloneAgentPicker"],
    ["New standalone terminal", "createStandaloneTerminal"],
    ["Add project", "openAddProject"],
    ["Clone a repository", "openCloneProject"],
    ["Initialize a repository", "openAddProjectForInit"],
  ])("%s closes the dialog and runs the shared action", (title, action) => {
    render(<NewLauncherDialog />)
    fireEvent.click(row(title))
    expect(calls).toEqual(["closeNewLauncher", action])
  })

  it("puts Projects first on a confirmed-empty workspace and disables what needs one", () => {
    seed({ projects: [], sessions: [] })
    render(<NewLauncherDialog />)
    expect(
      screen.getAllByRole("group").map((g) => g.getAttribute("aria-labelledby")),
    ).toEqual([
      "new-launcher-projects",
      "new-launcher-agents",
      "new-launcher-terminals",
    ])
    expect(
      screen.getByText("Add a project first, then start agents on it."),
    ).toBeTruthy()
    const disabled = (title: string) => (row(title) as HTMLButtonElement).disabled
    expect(disabled("New agent")).toBe(true)
    expect(disabled("New agent from PR")).toBe(true)
    expect(disabled("New agent from existing worktree")).toBe(true)
    expect(disabled("New terminal for an agent")).toBe(true)
    expect(disabled("New standalone agent")).toBe(false)
    expect(disabled("New standalone terminal")).toBe(false)
    expect(disabled("Add project")).toBe(false)
  })

  it("moves focus between rows with the arrow keys, skipping disabled ones", () => {
    seed({ projects: [], sessions: [] })
    render(<NewLauncherDialog />)
    row("Initialize a repository").focus()
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" })
    // The three disabled agent rows are stepped over.
    expect(document.activeElement).toBe(row("New standalone agent"))
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" })
    expect(document.activeElement).toBe(row("Initialize a repository"))
  })
})

describe("NewLauncherDialog agent step", () => {
  beforeEach(() =>
    seed({
      projects: [{ id: "p1", name: "demo-api" }],
      sessions: [
        managed("s1", "polish-onboarding", "p1"),
        managed("s2", "fix-login-redirect", "p1"),
        {
          id: "s3",
          title: "design-notes",
          status: "detached",
          working: false,
          workspace: {
            kind: "folder",
            folder_path: "/home/me/notes",
            folder_label: "~/notes",
          },
        },
      ],
    }),
  )

  it("lists every agent under its project, then the standalone ones, and narrows by name", () => {
    render(<NewLauncherDialog />)
    fireEvent.click(row("New terminal for an agent"))
    expect(screen.getByText("New terminal for an agent")).toBeTruthy()
    expect(
      screen.getAllByRole("group").map((g) => [
        g.getAttribute("aria-label"),
        Array.from(g.querySelectorAll("button")).map(
          (b) => b.querySelector(".truncate")?.textContent,
        ),
      ]),
    ).toEqual([
      ["demo-api", ["polish-onboarding", "fix-login-redirect"]],
      ["Standalone", ["design-notes"]],
    ])
    fireEvent.change(screen.getByRole("searchbox", { name: "Search agents" }), {
      target: { value: "LOGIN" },
    })
    expect(
      screen.getAllByRole("group").map((g) => g.getAttribute("aria-label")),
    ).toEqual(["demo-api"])
    // The match is lit in the name it matched.
    expect(screen.getByText("login").tagName).toBe("MARK")
  })

  it("opens the terminal for the picked agent, and Back returns to the list", () => {
    render(<NewLauncherDialog />)
    fireEvent.click(row("New terminal for an agent"))
    fireEvent.click(screen.getByRole("button", { name: "New…" }))
    expect(screen.getByText("Start something new")).toBeTruthy()
    fireEvent.click(row("New terminal for an agent"))
    fireEvent.click(screen.getByText("design-notes").closest("button")!)
    expect(calls).toEqual(["closeNewLauncher", "createTerminal(s3)"])
  })
})
