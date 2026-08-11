import { describe, expect, it } from "vitest"

import {
  agentHeaderChips,
  ASSISTANT_HINT,
  branchChip,
  captionText,
  directoryChip,
  focusedTerminalChip,
  headerChipTooltip,
  mobileHeaderLanes,
  terminalCountCaption,
  type AgentChipsInput,
} from "./headerSubject"

const AGENT: AgentChipsInput = {
  name: "server-mode",
  provider: "claude",
  projectName: "dux",
  branchName: "server-mode",
}

function kinds(input: AgentChipsInput) {
  return agentHeaderChips(input).map((c) => c.kind)
}

function chip(input: AgentChipsInput, kind: string) {
  return agentHeaderChips(input).find((c) => c.kind === kind)
}

describe("agentHeaderChips field order", () => {
  it("renders project, agent, branch, terminals, assistant in that order", () => {
    expect(
      kinds({
        ...AGENT,
        name: "menu-demo",
        branchName: "fix/paste-crash",
        terminalCount: 2,
      }),
    ).toEqual(["project", "agent", "branch", "terminal", "assistant"])
  })

  it("keeps the ordinary agent to project, agent, assistant", () => {
    expect(kinds(AGENT)).toEqual(["project", "agent", "assistant"])
  })

  it("omits the project chip when the project could not be resolved", () => {
    expect(kinds({ ...AGENT, projectName: undefined })).toEqual([
      "agent",
      "assistant",
    ])
  })
})

describe("agentHeaderChips values", () => {
  it("gives every field a glyph kind and its value", () => {
    const all = agentHeaderChips({
      ...AGENT,
      name: "menu-demo",
      branchName: "fix/paste-crash",
      terminalCount: 2,
    })
    expect(all.map((c) => [c.kind, c.value])).toEqual([
      ["project", "dux"],
      ["agent", "menu-demo"],
      ["branch", "fix/paste-crash"],
      ["terminal", "2"],
      ["assistant", "claude"],
    ])
  })

  it("names the FOCUSED tab's provider when one was passed", () => {
    expect(chip({ ...AGENT, provider: "codex" }, "assistant")?.value).toBe(
      "codex",
    )
  })
})

describe("the branch chip", () => {
  it("is absent when the branch merely repeats the agent name", () => {
    // The measured motivation: an untitled agent takes its name FROM its branch,
    // so the old bar printed that one word twice.
    expect(branchChip(AGENT)).toBeNull()
    expect(kinds(AGENT)).not.toContain("branch")
  })

  it("is present when the branch differs from the agent name", () => {
    const titled = { ...AGENT, name: "Tab redesign", branchName: "feature-x" }
    expect(branchChip(titled)?.value).toBe("feature-x")
    expect(kinds(titled)).toContain("branch")
  })

  it("compares exactly, so a prefix is not a collapse", () => {
    expect(branchChip({ ...AGENT, name: "server" })?.value).toBe("server-mode")
  })

  it("carries the drift note only when the initial branch truly differs", () => {
    const drifted = branchChip({
      ...AGENT,
      name: "Tab redesign",
      branchName: "now",
      initialBranch: "then",
    })
    expect(drifted?.hint).toBe("originally then")
    // An older server omits `initial_branch` entirely; nothing may render
    // "originally undefined".
    expect(
      branchChip({ ...AGENT, name: "Tab redesign", branchName: "now" })?.hint,
    ).toBeUndefined()
    expect(
      branchChip({
        ...AGENT,
        name: "Tab redesign",
        branchName: "now",
        initialBranch: "now",
      })?.hint,
    ).toBeUndefined()
  })

  it("appears for a DRIFTED branch even when it matches the agent name", () => {
    // Deliberately beyond the mock, which never draws a drifted agent. Without
    // this the drift note has no chip to live on and the fact is dropped
    // silently, which is worse than one extra chip in a rare case.
    const drifted = { ...AGENT, initialBranch: "main" }
    expect(branchChip(drifted)?.hint).toBe("originally main")
    expect(kinds(drifted)).toContain("branch")
  })
})

describe("the terminals chip", () => {
  it("appears only when the agent actually owns terminals", () => {
    expect(kinds({ ...AGENT, terminalCount: 0 })).not.toContain("terminal")
    expect(kinds({ ...AGENT })).not.toContain("terminal")
    expect(kinds({ ...AGENT, terminalCount: 1 })).toContain("terminal")
  })

  it("shows the bare count, since the glyph already says what is being counted", () => {
    expect(chip({ ...AGENT, terminalCount: 3 }, "terminal")?.value).toBe("3")
  })
})

describe("truncation priority", () => {
  it("makes the agent name the one chip that gives way LAST", () => {
    const all = agentHeaderChips({
      ...AGENT,
      name: "menu-demo",
      branchName: "fix/paste-crash",
      terminalCount: 2,
    })
    expect(all.filter((c) => c.primary).map((c) => c.kind)).toEqual(["agent"])
  })

  it("hands the primary slot to the TERMINAL when a terminal is on screen", () => {
    const owner = agentHeaderChips({ ...AGENT, primary: "none" })
    expect(owner.some((c) => c.primary)).toBe(false)
    expect(focusedTerminalChip("vim", 2).primary).toBe(true)
  })
})

describe("every chip names itself", () => {
  it("carries a non-empty hover label on every field of every variant", () => {
    const everyChip = [
      ...agentHeaderChips({
        ...AGENT,
        name: "menu-demo",
        branchName: "fix/paste-crash",
        terminalCount: 2,
      }),
      focusedTerminalChip("vim", 2),
      directoryChip("~/code"),
    ]
    for (const c of everyChip) {
      expect(c.label, `${c.kind} has no hover label`).toBeTruthy()
    }
    expect(everyChip.map((c) => c.label)).toEqual([
      "Project",
      "Agent",
      "Branch",
      "Terminals",
      "Assistant",
      "Terminal",
      "Directory",
    ])
  })

  it("points the assistant at where the provider can actually be changed", () => {
    expect(chip(AGENT, "assistant")?.hint).toBe(ASSISTANT_HINT)
  })
})

describe("headerChipTooltip", () => {
  const project = { kind: "project" as const, label: "Project", value: "dux" }

  it("says the field's name when the value is fully readable", () => {
    expect(headerChipTooltip(project, false)).toBe("Project")
  })

  it("adds the value only when it is actually cut off", () => {
    expect(headerChipTooltip(project, true)).toBe("Project · dux")
  })

  it("keeps the hint after the value", () => {
    const assistant = {
      kind: "assistant" as const,
      label: "Assistant",
      value: "claude",
      hint: ASSISTANT_HINT,
    }
    expect(headerChipTooltip(assistant, false)).toBe(
      `Assistant · ${ASSISTANT_HINT}`,
    )
    expect(headerChipTooltip(assistant, true)).toBe(
      `Assistant · claude · ${ASSISTANT_HINT}`,
    )
  })
})

describe("focusedTerminalChip", () => {
  it("names the terminal and keeps the sibling count in the hover clause", () => {
    expect(focusedTerminalChip("vim", 2)).toEqual({
      kind: "terminal",
      label: "Terminal",
      value: "vim",
      hint: "2 terminals",
      primary: true,
    })
  })

  it("says nothing about siblings when there are none", () => {
    expect(focusedTerminalChip("Terminal 1", 1).hint).toBeUndefined()
  })
})

describe("directoryChip", () => {
  it("names WHERE a standalone terminal is, since it has no owner to name", () => {
    expect(directoryChip("~/code")).toEqual({
      kind: "directory",
      label: "Directory",
      value: "~/code",
    })
  })
})

describe("captionText", () => {
  it("joins with a middot and drops empties", () => {
    expect(captionText(["dux", null, "claude", undefined, ""])).toBe(
      "dux · claude",
    )
  })
})

describe("mobileHeaderLanes", () => {
  it("puts the agent in lane one and project + assistant in lane two", () => {
    const lanes = mobileHeaderLanes({ ...AGENT, provider: "codex" })
    expect(lanes.lead.kind).toBe("agent")
    expect(lanes.lead.value).toBe("server-mode")
    expect(lanes.rest.map((c) => c.kind)).toEqual(["project", "assistant"])
    expect(lanes.rest.map((c) => c.value)).toEqual(["dux", "codex"])
  })

  // The phone deliberately drops branch and terminals: there is no hover on a
  // phone, so a glyph there could never explain itself.
  it("drops the branch and terminal chips the desktop row would carry", () => {
    const lanes = mobileHeaderLanes({
      ...AGENT,
      name: "menu-demo",
      branchName: "fix/paste-crash",
      terminalCount: 2,
    })
    expect(lanes.rest.map((c) => c.kind)).toEqual(["project", "assistant"])
  })

  it("degrades to the assistant alone when the project is unknown", () => {
    const lanes = mobileHeaderLanes({ ...AGENT, projectName: null })
    expect(lanes.rest.map((c) => c.kind)).toEqual(["assistant"])
  })

  // Reuses the desktop chip model rather than a second one, so the words a
  // chip carries cannot drift between the two surfaces.
  it("carries the same labels the desktop chips do", () => {
    const lanes = mobileHeaderLanes(AGENT)
    expect(lanes.lead.label).toBe("Agent")
    expect(lanes.rest.map((c) => c.label)).toEqual(["Project", "Assistant"])
  })
})

describe("terminalCountCaption", () => {
  it("pluralizes and omits zero", () => {
    expect(terminalCountCaption(0)).toBeNull()
    expect(terminalCountCaption(1)).toBe("1 terminal")
    expect(terminalCountCaption(3)).toBe("3 terminals")
  })
})
