import { describe, expect, it } from "vitest"

import type { AgentWorkspaceWire, RunningTabs } from "./agentWorkspace"
import {
  canRecreateWorkingCopy,
  recreateConfirmProse,
  recreateRestartCheckboxProse,
  recreateRunningTabClause,
  recreateView,
} from "./recreateWorkingCopy"
import { chip, proseText, quotedChip } from "./prose"

const recreateText = (...args: Parameters<typeof recreateConfirmProse>) =>
  proseText(recreateConfirmProse(...args))

/** Running tabs as the server sorts them. */
function running(over: Partial<RunningTabs>): RunningTabs {
  return { follows: [], restartable: [], left_running: [], ...over }
}

const managed: Extract<AgentWorkspaceWire, { kind: "managed" }> = {
  kind: "managed",
  project_id: "p1",
  branch_name: "feat",
  initial_branch: "feat",
  branch_provenance: "created",
  source_branch: "main",
  worktree_path: "/wt",
}

describe("canRecreateWorkingCopy", () => {
  it("is true only for a managed working copy that is gone", () => {
    expect(canRecreateWorkingCopy(managed)).toBe(false)
    expect(
      canRecreateWorkingCopy({ ...managed, worktree_missing: true }),
    ).toBe(true)
  })

  // dux never creates, moves or removes a standalone agent's folder.
  it("is false for a standalone agent whatever its folder looks like", () => {
    const folder: AgentWorkspaceWire = {
      kind: "folder",
      folder_path: "/home/someone/notes",
      folder_label: "~/notes",
      repo_status: "missing",
      quiet_reason: "gone",
    }
    expect(canRecreateWorkingCopy(folder)).toBe(false)
  })
})

// The web draws the path and every branch as a chip.
describe("the recreate body as names in prose", () => {
  it("marks the path and each branch", () => {
    const prose = recreateConfirmProse(
      "~/wt",
      "feat",
      "main",
      true,
      running({ restartable: ["codex"] }),
    )
    for (const name of [
      chip("~/wt"),
      quotedChip("feat"),
      quotedChip("origin/feat"),
      quotedChip("main"),
    ]) {
      expect(prose).toContainEqual(name)
    }
  })

  it("marks every provider named in the running-tab sentences", () => {
    const prose = recreateConfirmProse(
      "~/wt",
      "feat",
      "main",
      true,
      running({
        follows: ["claude"],
        restartable: ["codex", "opencode"],
        left_running: ["copilot"],
      }),
    )
    for (const name of ["Claude", "Codex", "Opencode", "Copilot"]) {
      expect(prose).toContainEqual(chip(name))
    }
  })
})

describe("the recreate body's words", () => {
  // Pinned to literal text here; the segments are pinned against the terminal
  // UI's by the shared fixture in prose.test.tsx.
  it("reads the same as the terminal UI's", () => {
    expect(
      recreateText(
        "~/worktrees/repo/feat",
        "feat",
        "main",
        true,
        running({ follows: ["claude"] }),
      ),
    ).toBe(
      "Recreate the working copy for this agent at ~/worktrees/repo/feat?\n\n" +
        'If branch "feat" still exists locally, dux checks it out there again. ' +
        'If it is gone locally but still on the remote, dux creates it again ' +
        'from "origin/feat", holding everything that had been pushed. If it is ' +
        'gone everywhere, dux creates it again from "main", and the commits ' +
        "that branch held are not coming back.\n\n" +
        "Any code changes that were in the old directory are gone either way: " +
        "this puts the directory back, not its contents. The conversation may " +
        "resume, because the agent's CLI keys its history by directory path and " +
        "dux recreates the working copy at the same path.\n\n" +
        "A running Claude tab keeps working in the recreated copy by itself. " +
        "A terminal still open in the old directory keeps " +
        "working in a directory that is gone; close it and open one in the " +
        "recreated copy.",
    )
  })

  // A provider with no directory-scoped resume (copilot ships with none) is
  // told so rather than promised a resume the same path cannot buy it.
  it("says the conversation will not resume when the provider cannot", () => {
    const body = recreateText(
      "~/wt",
      "feat",
      "main",
      false,
      running({ left_running: ["copilot"] }),
    )
    expect(body).toContain("will not resume")
    expect(body).not.toContain("may resume")
    expect(body).toContain("are gone either way")
  })

  // One sentence per fate, each naming every provider it is about: Claude
  // follows the folder, a CLI that can resume continues once restarted, and
  // one that cannot is left running with what to do about it.
  it("says what each running tab does", () => {
    expect(recreateRunningTabClause(running({ follows: ["claude"] }))).toBe(
      "A running Claude tab keeps working in the recreated copy by itself.",
    )
    expect(
      recreateRunningTabClause(
        running({ restartable: ["codex", "opencode", "codex"] }),
      ),
    ).toBe(
      "A running Codex or Opencode tab cannot follow the folder by itself; " +
        "restarting it continues its conversation in the recreated copy.",
    )
    expect(
      recreateRunningTabClause(running({ left_running: ["copilot"] })),
    ).toBe(
      "A running Copilot tab is left running, because restarting it would " +
        "lose its conversation; if it cannot follow the folder, stop it and " +
        "start it again yourself.",
    )
  })
})

describe("recreateRestartCheckboxProse", () => {
  it("names each provider once and reads plural by tab count", () => {
    expect(proseText(recreateRestartCheckboxProse(["codex"]))).toBe(
      "Restart the Codex tab so it continues in the new copy",
    )
    expect(
      proseText(recreateRestartCheckboxProse(["codex", "opencode"])),
    ).toBe(
      "Restart the Codex and Opencode tabs so they continue in the new copy",
    )
    expect(proseText(recreateRestartCheckboxProse(["codex", "codex"]))).toBe(
      "Restart the Codex tabs so they continue in the new copy",
    )
    expect(recreateRestartCheckboxProse(["codex", "opencode"])).toEqual([
      "Restart the ",
      chip("Codex"),
      " and ",
      chip("Opencode"),
      " tabs so they continue in the new copy",
    ])
  })
})

describe("recreateView", () => {
  // The server decides which CLI follows and which can resume; the browser
  // reads its answer.
  it("reads the server's sorted tabs and offer", () => {
    const view = {
      running: running({ restartable: ["codex"], left_running: ["copilot"] }),
      restart_offer: ["codex"],
    }
    expect(recreateView({ ...managed, recreate: view }, "claude")).toBe(view)
  })

  // A server that predates the field: the agent's own provider, no offer.
  it("falls back to the agent's provider with no restart offer", () => {
    expect(recreateView(managed, "claude")).toEqual({
      running: running({ follows: ["claude"] }),
      restart_offer: [],
    })
    expect(
      recreateView({ ...managed, conversation_resumes: false }, "grok"),
    ).toEqual({
      running: running({ follows: ["grok"] }),
      restart_offer: [],
    })
    expect(
      recreateView({ ...managed, conversation_resumes: true }, "codex"),
    ).toEqual({
      running: running({ restartable: ["codex"] }),
      restart_offer: [],
    })
    expect(
      recreateView({ ...managed, conversation_resumes: false }, "copilot"),
    ).toEqual({
      running: running({ left_running: ["copilot"] }),
      restart_offer: [],
    })
  })
})
