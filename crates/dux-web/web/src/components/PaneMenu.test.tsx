// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import { stubCoarsePointer, type MatchMediaStub } from "@/test/matchMedia"

// THE PANE'S ONE MENU. What is pinned here is that it is ONE menu: the docked
// flap, the floating pill and the desktop pane header open the same body under
// the same name, so theater cannot be the state in which the agent's own
// actions disappear, the cluster's `⋯` cannot mean something else once it has
// flown across the screen, and a computer's pane header cannot drift into a
// smaller menu than the row beside it.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

function installStubs() {
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
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}
installStubs()
const { MobileActionFlap } = await import("./MobileActionFlap")
const { TheaterPill } = await import("./TheaterPill")
const { PaneMenu, PANE_MENU_AGENT_LABEL, PANE_MENU_TERMINAL_GROUP_LABEL } =
  await import("./PaneMenu")
const { PANE_INPUT_GROUP_LABEL } = await import("./PaneInputGroup")
const { registerPaneInputGroup, resetPaneInputGroups } = await import(
  "@/lib/paneInputGroup"
)
const { registerAttachCapability, resetAttachCapabilities } = await import(
  "@/lib/attachRegistry"
)

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    title: "s1",
    status: "active",
    auto_reopen_enabled: false,
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/w",
    },
    tabs: [{ id: "s1", provider: "claude", order: 0 }],
  } as unknown as SessionView
}

const target = { kind: "agent" as const, sessionId: "s1", tabId: "s1" }

function makeState(theater: boolean): DuxState {
  return {
    spine: { projects: [{ id: "p1", name: "dux" }], sessions: [session()] },
    bootstrap: {
      title: "dux",
      agent_tabs_max: 4,
      available_providers: ["claude"],
      gh_available: false,
    },
    theater,
    createTabInFlight: [],
    changes: {
      sessionId: "s1",
      phase: "loaded",
      staged: [],
      unstaged: [{ path: "a" }],
    },
  } as unknown as DuxState
}

let media: MatchMediaStub | null = null
const desktopWidth = window.innerWidth

beforeEach(() => {
  installStubs()
  mockState = makeState(false)
  resetPaneInputGroups()
  resetAttachCapabilities()
})

afterEach(() => {
  cleanup()
  media?.restore()
  media = null
  Object.defineProperty(window, "innerWidth", {
    value: desktopWidth,
    configurable: true,
  })
  vi.unstubAllGlobals()
})

async function openFrom(el: Element) {
  fireEvent.click(el)
  await screen.findByRole("menu")
}

function labels() {
  return screen.getAllByRole("menuitem").map((item) => item.textContent)
}

describe("the phone's one pane menu", () => {
  it("carries the agent's own actions from the docked flap", async () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    const items = labels()
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    // NO "Changes ±N" ROW. The cluster this menu hangs off carries a real count
    // button, keyboard-reachable and labelled for a screen reader, and a second
    // copy of the number in the menu was two places for it to be printed.
    expect(items.some((t) => t?.startsWith("Changes"))).toBe(false)
    expect(
      screen.getByTestId("pane-changes-count").getAttribute("aria-label"),
    ).toBe("1 changed files")
    // The way to the app's own actions, named for the control it stands in for.
    expect(items.some((t) => t?.includes("Settings"))).toBe(true)
  })

  it("carries the same body from the floating pill, under the same name", async () => {
    mockState = makeState(true)
    render(
      <TheaterPill
        target={target}
        session={session()}
        variant="mobile"
        flight="floating"
      />,
    )
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    const items = labels()
    // The whole point of the merge: theater was the one state in which every
    // per-agent action was unreachable on a phone.
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    expect(items.some((t) => t?.startsWith("Changes"))).toBe(false)
    expect(items.some((t) => t?.includes("Settings"))).toBe(true)
  })

  it("offers the way out of theater only while the mode is on", async () => {
    mockState = makeState(true)
    render(
      <TheaterPill
        target={target}
        session={session()}
        variant="mobile"
        flight="floating"
      />,
    )
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    expect(labels().some((t) => t?.includes("Leave theater mode"))).toBe(true)
    // And nothing for the top bar: theater mode is the one way to hide the
    // phone's chrome, and a second flow for the same intent is exactly what
    // this menu must not grow back.
    expect(labels().some((t) => t?.includes("top bar"))).toBe(false)

    cleanup()
    mockState = makeState(false)
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    expect(labels().some((t) => t?.includes("Leave theater mode"))).toBe(false)
  })

  // THE INPUT GROUP IS THE PANE'S ANSWER, not this menu's. Typing directly in
  // the terminal takes the whole bottom bar away, so this menu is the only
  // permanent home the virtual input's controls have; what is in it comes from
  // the mounted owner pane, which is the only thing that knows.
  it("carries the pane's INPUT group, at the top, from what the pane publishes", async () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    media = stubCoarsePointer()
    registerPaneInputGroup("s1", { surfaceSwitch: true, keysToggle: false })
    registerAttachCapability("s1", vi.fn())
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    const items = labels()
    expect(items[0]).toContain("Attach a file…")
    expect(items[1]).toContain("Use virtual input")
    // The label stays even with one item, and it is above the agent's actions.
    const group = screen.getByText(PANE_INPUT_GROUP_LABEL)
    const rename = screen
      .getAllByRole("menuitem")
      .find((el) => el.textContent?.includes("Rename agent…"))!
    expect(
      group.compareDocumentPosition(rename) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
    // And nothing for the top bar: theater mode is the one way to hide the
    // phone's chrome, and a second flow for the same intent is exactly what
    // this menu must not grow back.
    expect(items.some((t) => t?.includes("top bar"))).toBe(false)
  })

  // The way BACK only. While the virtual input is up, the bottom `⋯` inside it
  // owns the other direction, and offering both here is how the two would
  // eventually disagree about which surface is live.
  it("has no INPUT group at all when the pane publishes nothing", async () => {
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    expect(screen.queryByText(PANE_INPUT_GROUP_LABEL)).toBeNull()
    expect(labels().some((t) => t?.includes("Attach a file…"))).toBe(false)
    expect(labels().some((t) => t?.includes("Use virtual input"))).toBe(false)
  })
})

// A COMPANION TERMINAL'S PANE wears its agent's menu, because the header, the
// count and the PR chip around it are that agent's. What it must not lose in
// the bargain is its own verbs: the sidebar row that used to be their only
// other home is exactly what a narrow window and theater take away.
describe("an agent's menu over one of its companion terminals", () => {
  const terminalPane = {
    kind: "terminal" as const,
    terminalId: "t7",
    owner: { kind: "session" as const, sessionId: "s1" },
  }

  it("carries the terminal's own verbs under a heading, below the agent's", async () => {
    render(
      <PaneMenu
        subject={{ kind: "agent", session: session() }}
        pane={terminalPane}
        appearance="header"
        settingsDrill={false}
      />,
    )
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    const items = labels()
    // The agent's, because this header is the agent's.
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    // And the terminal's, which had nowhere else to be reached from.
    expect(items.some((t) => t?.includes("Close…"))).toBe(true)
    expect(items.some((t) => t?.includes("Open editor in new tab"))).toBe(true)
    // Labelled, so Close… cannot read as one more agent action.
    const heading = screen.getByText(PANE_MENU_TERMINAL_GROUP_LABEL)
    const remove = screen
      .getAllByRole("menuitem")
      .find((el) => el.textContent?.includes("Delete agent…"))!
    // After the agent's actions: the agent body keeps the row order it has at
    // every other anchor, and the terminal's group is what is added.
    expect(
      remove.compareDocumentPosition(heading) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  })

  it("adds nothing when the pane IS the agent's own tab", async () => {
    render(
      <PaneMenu
        subject={{ kind: "agent", session: session() }}
        pane={target}
        appearance="header"
        settingsDrill={false}
      />,
    )
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    expect(screen.queryByText(PANE_MENU_TERMINAL_GROUP_LABEL)).toBeNull()
    expect(labels().some((t) => t?.includes("Close…"))).toBe(false)
  })

  it("keeps a terminal's own menu unlabelled, where every row is already the terminal's", async () => {
    render(
      <PaneMenu
        subject={{
          kind: "terminal",
          terminalId: "t9",
          owner: { kind: "standalone" },
        }}
        pane={{
          kind: "terminal",
          terminalId: "t9",
          owner: { kind: "standalone" },
        }}
        appearance="header"
        settingsDrill={false}
      />,
    )
    await openFrom(screen.getByLabelText("Terminal actions"))
    expect(labels().some((t) => t?.includes("Close…"))).toBe(true)
    expect(screen.queryByText(PANE_MENU_TERMINAL_GROUP_LABEL)).toBeNull()
  })
})

// THE COMPUTER'S ANCHORS. The desktop pane header's `⋯` is the same menu the
// phone's flap opens, in a trigger that matches the header's cluster instead of
// the flap's bare circle, and the pill it hands over to in theater opens it too.
// Two anchors, one body: the header and the sidebar row must never be two
// different menus about one agent.
describe("the pane menu at the computer's anchors", () => {
  // THE COMPUTER'S PILL, under the same rule. Theater unmounts the sidebar and
  // the header together, so the desktop pill carrying only the app menu left a
  // computer in this mode with no route to the agent's own actions at all,
  // which is exactly the gap the phone's merge closed.
  it("carries the same body from the desktop pill, with the app menu inside it", async () => {
    mockState = makeState(true)
    registerPaneInputGroup("s1", { surfaceSwitch: true, keysToggle: false })
    render(<TheaterPill target={target} session={session()} />)
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    const items = labels()
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Use virtual input"))).toBe(true)
    expect(items.some((t) => t?.includes("Leave theater mode"))).toBe(true)
    // The app menu is still reachable, as the drill named for the cog the mode
    // took away.
    expect(items.some((t) => t?.includes("Settings"))).toBe(true)
  })

  it("opens the whole agent menu, not a header-sized subset", async () => {
    registerPaneInputGroup("s1", { surfaceSwitch: true, keysToggle: false })
    render(<PaneMenu
        subject={{ kind: "agent", session: session() }}
        appearance="header"
        settingsDrill={false}
      />)
    await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
    const items = labels()
    expect(items.some((t) => t?.includes("Use virtual input"))).toBe(true)
    expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
    expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    // Everything except the drill: the cog is in this very header, so the whole
    // menu here is the whole PANE menu and not a second copy of the app's.
    expect(items.some((t) => t?.includes("Settings"))).toBe(false)
    expect(screen.getByText(PANE_INPUT_GROUP_LABEL)).toBeTruthy()
  })

  it("names itself what every other anchor names itself", () => {
    // One name across the surfaces, so a screen reader and a voice command do
    // not have to learn which anchor is on screen.
    render(<PaneMenu
        subject={{ kind: "agent", session: session() }}
        appearance="header"
        settingsDrill={false}
      />)
    expect(screen.getByLabelText(PANE_MENU_AGENT_LABEL)).toBeTruthy()
  })

  it("wears the header cluster's treatment, not the flap's circle", () => {
    // The cluster is one family (outline) at one height token; the flap and the
    // pill are each ONE rounded surface, where a bordered button reads as two.
    render(<PaneMenu
        subject={{ kind: "agent", session: session() }}
        appearance="header"
        settingsDrill={false}
      />)
    const trigger = screen.getByLabelText(PANE_MENU_AGENT_LABEL)
    expect(trigger.className).toContain("size-8")
    expect(trigger.className).not.toContain("rounded-full")

    cleanup()
    render(<MobileActionFlap target={target} subject={{ kind: "agent", session: session() }} band="strip" />)
    const clusterTrigger = screen.getByLabelText(PANE_MENU_AGENT_LABEL)
    expect(clusterTrigger.className).toContain("size-10")
    expect(clusterTrigger.className).toContain("rounded-full")
  })
})

// THE SETTINGS DRILL RENDERS ONLY WHERE THE APP-MENU COG IS NOT ON SCREEN.
//
// One table per anchor, because the rule is about the chrome AROUND the anchor
// and nothing inside the menu can see it. The sidebar and hub rows are the same
// component at both widths and are covered where that component is rendered
// whole (`FlatAgentList.test.tsx`); everything else that anchors this menu is
// here.
describe("the Settings drill, per anchor", () => {
  const anchors: {
    name: string
    drill: boolean
    render: () => void
  }[] = [
    {
      // A phone pane screen's header is Back and identity: no cog travelled
      // with it from the hub.
      name: "the phone's docked flap",
      drill: true,
      render: () =>
        render(
          <MobileActionFlap
            target={target}
            subject={{ kind: "agent", session: session() }}
            band="strip"
          />,
        ),
    },
    {
      // Theater unmounts the top bar on a phone and the header stack on a
      // computer, and the cog goes with it either way.
      name: "the floating pill on a phone",
      drill: true,
      render: () => {
        mockState = makeState(true)
        render(
          <TheaterPill
            target={target}
            session={session()}
            variant="mobile"
            flight="floating"
          />,
        )
      },
    },
    {
      name: "the floating pill on a computer",
      drill: true,
      render: () => {
        mockState = makeState(true)
        render(<TheaterPill target={target} session={session()} />)
      },
    },
    {
      // The cog is mounted in this very header, a couple of controls to the
      // right, for as long as this `⋯` exists.
      name: "the desktop pane header",
      drill: false,
      render: () =>
        render(
          <PaneMenu
            subject={{ kind: "agent", session: session() }}
            appearance="header"
            settingsDrill={false}
          />,
        ),
    },
  ]

  for (const anchor of anchors) {
    it(`${anchor.drill ? "drills into the app menu from" : "leaves the app menu to the cog at"} ${anchor.name}`, async () => {
      anchor.render()
      await openFrom(screen.getByLabelText(PANE_MENU_AGENT_LABEL))
      const items = labels()
      expect(items.some((t) => t?.includes("Settings"))).toBe(anchor.drill)
      // NOTHING ELSE IN THE BODY MOVES WITH IT: the gate is the last row, not a
      // second menu.
      expect(items.some((t) => t?.includes("Rename agent…"))).toBe(true)
      expect(items.some((t) => t?.includes("Delete agent…"))).toBe(true)
    })
  }
})
