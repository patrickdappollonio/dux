import { describe, expect, it } from "vitest"

import {
  agentCaption,
  agentHeaderSubject,
  branchCaption,
  captionText,
  mobileCaption,
  SAME_BRANCH_CAPTION,
  terminalCountCaption,
  terminalHeaderSubject,
} from "./headerSubject"

describe("branchCaption", () => {
  it("collapses to `same branch` when the branch repeats the subject name", () => {
    // The measured motivation: an agent named after its branch printed that one
    // word twice in a bar of about 74 characters.
    expect(branchCaption("server-mode", "server-mode")).toBe(SAME_BRANCH_CAPTION)
  })

  it("shows the branch when it differs from the subject name", () => {
    expect(branchCaption("tabs-redesign", "server-mode")).toBe("server-mode")
  })

  it("compares exactly, so a prefix is not a collapse", () => {
    expect(branchCaption("server", "server-mode")).toBe("server-mode")
  })
})

describe("agentCaption", () => {
  it("orders project, provider, branch clause", () => {
    expect(
      agentCaption({
        name: "tabs",
        provider: "claude",
        projectName: "dux",
        branchName: "tabs-work",
      }),
    ).toEqual(["dux", "claude", "tabs-work"])
  })

  it("carries `same branch` in place of the repeated name", () => {
    expect(
      agentCaption({
        name: "server-mode",
        provider: "claude",
        projectName: "dux",
        branchName: "server-mode",
      }),
    ).toEqual(["dux", "claude", SAME_BRANCH_CAPTION])
  })

  it("omits the project when it could not be resolved", () => {
    expect(
      agentCaption({ name: "a", provider: "codex", branchName: "b" }),
    ).toEqual(["codex", "b"])
  })

  it("appends the drift clause only when the initial branch truly differs", () => {
    expect(
      agentCaption({
        name: "a",
        provider: "codex",
        branchName: "now",
        initialBranch: "then",
      }),
    ).toContain("originally then")
    // An older server omits `initial_branch` entirely; nothing may render
    // "originally undefined".
    expect(
      agentCaption({ name: "a", provider: "codex", branchName: "now" }).join(),
    ).not.toMatch(/originally/)
    expect(
      agentCaption({
        name: "a",
        provider: "codex",
        branchName: "now",
        initialBranch: "now",
      }).join(),
    ).not.toMatch(/originally/)
  })
})

describe("agentHeaderSubject", () => {
  it("makes the agent name the subject and everything else the caption", () => {
    expect(
      agentHeaderSubject({
        name: "server-mode",
        provider: "claude",
        projectName: "dux",
        branchName: "server-mode",
      }),
    ).toEqual({
      subject: "server-mode",
      caption: ["dux", "claude", SAME_BRANCH_CAPTION],
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

describe("mobileCaption", () => {
  it("names the project and the provider, the two facts the phone header lacked", () => {
    expect(mobileCaption({ provider: "codex", projectName: "dux" })).toBe(
      "dux · codex",
    )
  })

  it("degrades to the provider alone when the project is unknown", () => {
    expect(mobileCaption({ provider: "codex" })).toBe("codex")
  })
})

describe("terminalCountCaption", () => {
  it("pluralizes and omits zero", () => {
    expect(terminalCountCaption(0)).toBeNull()
    expect(terminalCountCaption(1)).toBe("1 terminal")
    expect(terminalCountCaption(3)).toBe("3 terminals")
  })
})

describe("terminalHeaderSubject", () => {
  it("makes the terminal the subject and its owner the caption", () => {
    expect(terminalHeaderSubject("vim", ["dux"], 2)).toEqual({
      subject: "vim",
      caption: ["dux", "2 terminals"],
    })
  })

  it("drops the count clause at zero rather than saying `0 terminals`", () => {
    expect(terminalHeaderSubject("Terminal 1", ["~/code"], 0)).toEqual({
      subject: "Terminal 1",
      caption: ["~/code"],
    })
  })
})
