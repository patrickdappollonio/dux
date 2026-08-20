import { describe, expect, it } from "vitest"

import {
  type AgentWorkspaceWire,
  type FolderRepoStatus,
  changesPanelWorks,
  changesQuietReason,
  folderWorkspace,
  managedWorkspace,
  matchWorkspace,
  supportsBranchGit,
  workspaceBranchName,
  workspaceDirectory,
  workspaceLocation,
  workspaceProjectId,
} from "./agentWorkspace"

const managed: AgentWorkspaceWire = {
  kind: "managed",
  project_id: "p1",
  branch_name: "feature/x",
  initial_branch: "feature/x",
  branch_provenance: "created",
  source_branch: "main",
  worktree_path: "/managed/wt",
}

function folder(
  repo_status: FolderRepoStatus,
  quiet_reason = "because",
): AgentWorkspaceWire {
  return {
    kind: "folder",
    folder_path: "/home/someone/notes",
    folder_label: "~/notes",
    repo_status,
    quiet_reason,
  }
}

describe("agent workspace", () => {
  it("gives a managed agent every git answer", () => {
    expect(supportsBranchGit(managed)).toBe(true)
    expect(workspaceProjectId(managed)).toBe("p1")
    expect(workspaceBranchName(managed)).toBe("feature/x")
    expect(workspaceDirectory(managed)).toBe("/managed/wt")
    expect(managedWorkspace(managed)).not.toBeNull()
    expect(folderWorkspace(managed)).toBeNull()
  })

  // The whole point of the either/or: a standalone agent has no branch, and
  // asking for one gets an honest null rather than an empty string some screen
  // renders as a branch named "".
  it("gives a standalone agent no branch identity at all", () => {
    const workspace = folder("no_repo")
    expect(supportsBranchGit(workspace)).toBe(false)
    expect(workspaceProjectId(workspace)).toBeNull()
    expect(workspaceBranchName(workspace)).toBeNull()
    expect(managedWorkspace(workspace)).toBeNull()
    expect(workspaceDirectory(workspace)).toBe("/home/someone/notes")
  })

  // The branch features and the changes panel ask DIFFERENT questions: a
  // standalone agent pointed at a repository gets a real changes panel and
  // still no fork, no pull request, no push.
  it("lets the changes panel work in a repository folder while the branch features still do not exist", () => {
    const workspace = folder("working_repo")
    expect(changesPanelWorks(workspace)).toBe(true)
    expect(changesQuietReason(workspace)).toBeNull()
    expect(supportsBranchGit(workspace)).toBe(false)
  })

  it("keeps every other folder quiet, with its own reason", () => {
    for (const status of [
      "inside_repo_rooted_elsewhere",
      "no_repo",
      "indeterminate",
    ] as const) {
      const workspace = folder(status, `quiet because ${status}`)
      expect(changesPanelWorks(workspace)).toBe(false)
      expect(changesQuietReason(workspace)).toBe(`quiet because ${status}`)
    }
  })

  it("names the project for a managed agent and the folder for a standalone one", () => {
    expect(workspaceLocation(managed)).toEqual({
      kind: "project",
      projectId: "p1",
    })
    expect(workspaceLocation(folder("no_repo"))).toEqual({
      kind: "folder",
      label: "~/notes",
    })
  })

  it("routes every decision through an exhaustive matcher", () => {
    const label = (workspace: AgentWorkspaceWire) =>
      matchWorkspace(workspace, {
        managed: (w) => `branch ${w.branch_name}`,
        folder: (w) => `folder ${w.folder_label}`,
      })
    expect(label(managed)).toBe("branch feature/x")
    expect(label(folder("no_repo"))).toBe("folder ~/notes")
  })

  // A workspace kind from a NEWER server. The matcher throws rather than
  // guessing: guessing "managed" would hand a directory dux does not
  // understand to code that believes it may delete it, and guessing "folder"
  // would hide every git affordance from an agent that has them.
  it("refuses to guess at a workspace kind it has never heard of", () => {
    const future = { kind: "something-new" } as unknown as AgentWorkspaceWire
    expect(() =>
      matchWorkspace(future, {
        managed: () => "managed",
        folder: () => "folder",
      }),
    ).toThrow()
  })
})
