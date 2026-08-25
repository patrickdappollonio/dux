import { describe, expect, it } from "vitest"

import {
  type AgentWorkspaceWire,
  type FolderRepoStatus,
  changesQuietReason,
  folderWorkspace,
  managedWorkspace,
  matchWorkspace,
  sessionLabel,
  supportsBranchGit,
  workspaceBranchName,
  workspaceDirectory,
  workspaceLocation,
  workspaceProjectId, folderDisplayName } from "./agentWorkspace"

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
    // A null quiet reason IS the working case; there is no second predicate.
    expect(changesQuietReason(workspace)).toBeNull()
    expect(supportsBranchGit(workspace)).toBe(false)
  })

  it("keeps every other folder quiet, with its own reason", () => {
    for (const status of [
      "inside_repo_rooted_elsewhere",
      "no_repo",
      "indeterminate",
      // Nobody has looked yet: quiet like the rest, with its own sentence.
      "unprobed",
    ] as const) {
      const workspace = folder(status, `quiet because ${status}`)
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

  // A workspace kind from a NEWER server never reaches the matcher: ingestion
  // degrades it to the managed shape first (see workspaceApi's normalization
  // and its own test). The throw stays because it is what makes a missing case
  // a compile error, and it is asserted here as the last line of defence for a
  // hand-built workspace that skipped ingestion, NOT as the behaviour a real
  // newer server produces.
  // ── SHARED VECTORS with dux-core `model.rs`
  // `display_label_names_a_standalone_agent_after_its_folder` ────────────────
  //
  // The two surfaces name one agent, so they must name it the same thing. The
  // path cases are the ones a split-on-slash gets wrong, measured on the Rust
  // side against `Path::file_name`.
  it("names a standalone agent exactly as dux-core's display_label does", () => {
    const withFolder = (folder_path: string, title: string | null = null) => ({
      title,
      workspace: {
        kind: "folder" as const,
        folder_path,
        folder_label: "~/elsewhere",
        repo_status: "working_repo" as const,
        quiet_reason: "",
      },
    })

    // A title always wins, whatever the folder is called.
    expect(sessionLabel(withFolder("/home/someone/notes", "My notes"))).toBe(
      "My notes",
    )
    expect(sessionLabel(withFolder("/home/someone/notes"))).toBe("notes")
    // A trailing slash names the same folder.
    expect(sessionLabel(withFolder("/home/someone/notes/"))).toBe("notes")
    // A trailing "." is not a name of its own.
    expect(sessionLabel(withFolder("/home/someone/notes/."))).toBe("notes")
    // A path whose last component is ".." has no name at all, so the label
    // falls back to the whole path rather than the word "..".
    expect(sessionLabel(withFolder("/home/someone/notes/.."))).toBe(
      "/home/someone/notes/..",
    )
    // Nor has the root, nor the empty string. Both fall back to the same field
    // the Rust side falls back to, the path itself.
    expect(sessionLabel(withFolder("/"))).toBe("/")
    expect(sessionLabel(withFolder(""))).toBe("")

    // A managed agent still takes its branch.
    expect(sessionLabel({ title: null, workspace: managed })).toBe("feature/x")
  })

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

describe("folderDisplayName", () => {
  it("keeps only the folder's last component", () => {
    expect(folderDisplayName("~/work/notes")).toBe("notes")
    expect(folderDisplayName("/srv/app")).toBe("app")
    expect(folderDisplayName("~/design-notes/")).toBe("design-notes")
  })
  it("writes home as $HOME and keeps the root", () => {
    expect(folderDisplayName("~")).toBe("$HOME")
    expect(folderDisplayName("/")).toBe("/")
    expect(folderDisplayName("")).toBe("")
  })
})
