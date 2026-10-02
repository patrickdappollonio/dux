import { describe, expect, it } from "vitest"

import {
  type AgentWorkspaceWire,
  type FolderRepoStatus,
  MISSING_FOLDER_INFO_LINE,
  MISSING_WORKING_COPY_INFO_LINE,
  changesQuietReason,
  changesGitBlockedReason,
  directoryGoneReason,
  missingDirectoryInfoLine,
  folderWorkspace,
  managedWorkspace,
  matchWorkspace,
  newTerminalLabel,
  sessionLabel,
  supportsBranchGit,
  workspaceBranchName,
  workspaceDirectory,
  workspaceLocation,
  workspaceProjectId, folderDisplayName, workingCopyMissing } from "./agentWorkspace"

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

const missingCopy: AgentWorkspaceWire = {
  ...managed,
  worktree_missing: true,
  quiet_reason: "The working copy at /managed/wt no longer exists on disk.",
}

describe("a working copy that is gone", () => {
  // Its own verdict, never "git is busy": there is no directory to run git in.
  it("makes the changes region quiet with the server's own sentence", () => {
    expect(changesQuietReason(missingCopy)).toBe(missingCopy.quiet_reason)
    expect(workingCopyMissing(missingCopy)).toBe(true)
  })

  it("leaves a healthy managed agent exactly as it was", () => {
    expect(changesQuietReason(managed)).toBeNull()
    expect(workingCopyMissing(managed)).toBe(false)
  })

  // A server that predates the field sends neither, and absent must read as
  // "the working copy is there", which is what the server itself answers before
  // its own probe lands.
  it("reads an absent flag as present", () => {
    const older: AgentWorkspaceWire = { ...managed }
    expect(workingCopyMissing(older)).toBe(false)
    expect(changesQuietReason(older)).toBeNull()
  })

  // dux never creates, moves or removes a standalone agent's folder, so a
  // missing one is a sentence and never the recreate button.
  it("is never claimed for a standalone agent's own folder", () => {
    expect(workingCopyMissing(folder("missing", "the folder is gone"))).toBe(false)
    expect(changesQuietReason(folder("missing", "the folder is gone"))).toBe(
      "the folder is gone",
    )
  })
})

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

describe("newTerminalLabel", () => {
  it("names the worktree for a managed agent and the folder for a standalone one", () => {
    expect(newTerminalLabel(managed)).toBe("New terminal in the worktree")
    expect(newTerminalLabel(folder("working_repo"))).toBe(
      "New terminal in the folder",
    )
  })

  it("never says worktree about a folder, whatever git makes of it", () => {
    for (const status of [
      "working_repo",
      "inside_repo_rooted_elsewhere",
      "no_repo",
      "indeterminate",
      "unprobed",
    ] as FolderRepoStatus[]) {
      expect(newTerminalLabel(folder(status))).not.toMatch(/worktree/)
    }
  })
})

describe("missingDirectoryInfoLine", () => {
  // Pinned against `dux_core::working_copy`, which carries the twin assertion,
  // so a wording change fails on whichever side changed.
  it("reads the same as the terminal UI's info panel", () => {
    expect(MISSING_WORKING_COPY_INFO_LINE).toBe(
      "The working copy no longer exists on disk. Recreate it to get this agent running again.",
    )
    expect(MISSING_FOLDER_INFO_LINE).toBe(
      "The folder no longer exists on disk. Restore it, or delete this agent.",
    )
  })

  it("names the remedy that belongs to the kind, and is silent otherwise", () => {
    expect(missingDirectoryInfoLine(missingCopy)).toBe(
      MISSING_WORKING_COPY_INFO_LINE,
    )
    expect(
      missingDirectoryInfoLine({ ...missingCopy, worktree_missing: false }),
    ).toBeNull()
    expect(
      missingDirectoryInfoLine(folder("missing")),
    ).toBe(MISSING_FOLDER_INFO_LINE)
    expect(
      missingDirectoryInfoLine(
        folder("no_repo"),
      ),
    ).toBeNull()
  })
})

// The Changes pane keeps its header and menu in every quiet state, and greys
// out the git items there with a short reason. The reason is the first clause
// of the server's own quiet sentence, so the menu and the body below it say
// the same thing, and it exists exactly when the panel is quiet.
describe("why the Changes pane's git items are unavailable", () => {
  it("names each quiet folder state in a short sentence", () => {
    expect(changesGitBlockedReason(folder("no_repo"))).toBe(
      "This folder has no git repository.",
    )
    expect(changesGitBlockedReason(folder("inside_repo_rooted_elsewhere"))).toBe(
      "This folder sits inside a repository rooted elsewhere.",
    )
    expect(changesGitBlockedReason(folder("indeterminate"))).toBe(
      "dux could not consult git about this folder.",
    )
    expect(changesGitBlockedReason(folder("unprobed"))).toBe(
      "dux is still looking at this folder.",
    )
    expect(changesGitBlockedReason(folder("missing"))).toBe(
      "This folder no longer exists on disk.",
    )
  })

  it("names a managed working copy that is gone", () => {
    expect(changesGitBlockedReason(missingCopy)).toBe(
      "This working copy no longer exists on disk.",
    )
  })

  it("has nothing to say where the panel works", () => {
    expect(changesGitBlockedReason(folder("working_repo"))).toBeNull()
    expect(changesGitBlockedReason(managed)).toBeNull()
  })

  // One spelling of "is the panel quiet": the short reason follows
  // `changesQuietReason`, so an older server that sends no quiet sentence for a
  // managed agent never greys out its menu.
  it("follows the quiet verdict rather than asking a second question", () => {
    const older: AgentWorkspaceWire = { ...managed, worktree_missing: true }
    expect(changesQuietReason(older)).toBeNull()
    expect(changesGitBlockedReason(older)).toBeNull()
  })
})

// The editor is rooted at the agent's directory, so it opens in every quiet
// state but one: the directory itself being gone.
describe("why an agent's directory cannot be opened in the editor", () => {
  it("is only the directory being gone", () => {
    expect(directoryGoneReason(folder("missing"))).toBe(
      "This folder no longer exists on disk.",
    )
    expect(directoryGoneReason(missingCopy)).toBe(
      "This working copy no longer exists on disk.",
    )
    for (const status of [
      "working_repo",
      "inside_repo_rooted_elsewhere",
      "no_repo",
      "indeterminate",
      "unprobed",
    ] as const) {
      expect(directoryGoneReason(folder(status)), status).toBeNull()
    }
    expect(directoryGoneReason(managed)).toBeNull()
  })
})

// An empty sentence is no sentence: the pane's body and its menu read the one
// verdict, so neither can call the pane quiet while the other does not.
describe("an empty quiet sentence", () => {
  it("counts as not quiet for both the body and the menu", () => {
    const empty = folder("no_repo", "")
    expect(changesQuietReason(empty)).toBeNull()
    expect(changesGitBlockedReason(empty)).toBeNull()
    const emptyManaged: AgentWorkspaceWire = {
      ...managed,
      worktree_missing: true,
      quiet_reason: "",
    }
    expect(changesQuietReason(emptyManaged)).toBeNull()
    expect(changesGitBlockedReason(emptyManaged)).toBeNull()
  })
})
