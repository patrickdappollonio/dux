// Where an agent lives, defined ONCE and switched on exhaustively: a working
// copy dux created and owns, or a folder the user already had. Every git field
// belongs to the first shape and does not exist in the second, so the wire value
// is TAGGED rather than a flat record padded with empty strings that some screen
// eventually renders. Mirrors `lib/terminalOwner.ts` down to the
// matcher-plus-`assertNever` shape; the Rust side is
// `dux_core::viewmodel::AgentWorkspaceView`.

import { assertNever } from "@/lib/assertNever"

/** The repository verdict for a standalone agent's folder, decided on the
 * server (it shells out to git) so both surfaces render the same answer the
 * server acted on. Only `working_repo` gets a real changes panel; the rest are
 * quiet, and `quiet_reason` says which quiet this is. */
export type FolderRepoStatus =
  | "working_repo"
  | "inside_repo_rooted_elsewhere"
  | "no_repo"
  | "indeterminate"
  /** Nobody has looked yet. Gates exactly as `indeterminate` does; its
   * `quiet_reason` reads as a wait rather than as a fault, because a freshly
   * created agent in a healthy repository spends a moment here. */
  | "unprobed"

/** The serialized workspace, exactly as it arrives on `SessionView.workspace`.
 * Field names are the server's (snake_case). */
export type AgentWorkspaceWire =
  | {
      kind: "managed"
      project_id: string
      branch_name: string
      initial_branch: string
      branch_provenance: "created" | "attached" | "adopted" | "unknown"
      source_branch: string
      worktree_path: string
    }
  | {
      kind: "folder"
      /** The folder as it exists on the SERVER's filesystem. */
      folder_path: string
      /** The same folder with the server's home directory collapsed to `~`.
       * Shortened server-side because the browser is not necessarily on the
       * same machine and has no `~` of the server's to collapse against. */
      folder_label: string
      repo_status: FolderRepoStatus
      /** Why the changes region is quiet, in the user's terms. Authored
       * server-side so the terminal UI and the web say the same thing. */
      quiet_reason: string
    }

/** A handler per workspace variant. A `switch` inside a helper only protects
 * the helper, so a consumer whose behaviour depends on the kind of agent takes
 * one of these literals and a missing key is a compile error there. */
export type WorkspaceMatch<T> = {
  [K in AgentWorkspaceWire["kind"]]: (
    workspace: Extract<AgentWorkspaceWire, { kind: K }>,
  ) => T
}

export function matchWorkspace<T>(
  workspace: AgentWorkspaceWire,
  on: WorkspaceMatch<T>,
): T {
  switch (workspace.kind) {
    case "managed":
      return on.managed(workspace)
    case "folder":
      return on.folder(workspace)
    default:
      return assertNever(workspace)
  }
}

/** The managed payload, or `null` for a standalone agent. LOSSY ON PURPOSE, for
 * sites where "does this agent have a branch at all" is the whole question; a
 * site whose behaviour differs per kind uses `matchWorkspace`, so a third kind
 * is a compile error there. */
export function managedWorkspace(
  workspace: AgentWorkspaceWire,
): Extract<AgentWorkspaceWire, { kind: "managed" }> | null {
  return matchWorkspace(workspace, {
    managed: (w) => w,
    folder: () => null,
  })
}

/** The folder payload, or `null` for a managed agent. Same lossy contract as
 * `managedWorkspace`, from the other side. */
export function folderWorkspace(
  workspace: AgentWorkspaceWire,
): Extract<AgentWorkspaceWire, { kind: "folder" }> | null {
  return matchWorkspace(workspace, {
    managed: () => null,
    folder: (w) => w,
  })
}

/** Whether the branch-identity features exist here: fork, pull requests, push,
 * pull, branch rename and display, provenance, the worktree manager. They are
 * about a branch dux manages, and a standalone agent has none whatever its
 * folder contains. The changes panel asks `changesQuietReason` instead, folder
 * driven, because a folder at a repository's top level gets a real panel. */
export function supportsBranchGit(workspace: AgentWorkspaceWire): boolean {
  return managedWorkspace(workspace) !== null
}

/** Why the changes region is quiet, or `null` when it is not, which doubles as
 * "does the changes panel work here". Deliberately the only spelling of that
 * question: a second one is how the two answers drift. */
export function changesQuietReason(
  workspace: AgentWorkspaceWire,
): string | null {
  return matchWorkspace(workspace, {
    managed: () => null,
    folder: (w) => (w.repo_status === "working_repo" ? null : w.quiet_reason),
  })
}

/** The project this agent belongs to, or `null` for a standalone agent, which
 * belongs to none. */
export function workspaceProjectId(
  workspace: AgentWorkspaceWire,
): string | null {
  return matchWorkspace(workspace, {
    managed: (w) => w.project_id,
    folder: () => null,
  })
}

/** The branch this agent tracks, or `null` when it has none. */
export function workspaceBranchName(
  workspace: AgentWorkspaceWire,
): string | null {
  return matchWorkspace(workspace, {
    managed: (w) => w.branch_name,
    folder: () => null,
  })
}

/** The directory this agent occupies: its worktree, or the user's folder. Both
 * shapes have one, so this is what a consumer that only needs a working
 * directory should ask for. It is NOT a promise that git can run there. */
export function workspaceDirectory(workspace: AgentWorkspaceWire): string {
  return matchWorkspace(workspace, {
    managed: (w) => w.worktree_path,
    folder: (w) => w.folder_path,
  })
}

/** What the agent menu's terminal entry says. A companion terminal opens in the
 * agent's own directory, so naming the worktree for both kinds would promise a
 * standalone agent something it does not have. */
export function newTerminalLabel(workspace: AgentWorkspaceWire): string {
  return matchWorkspace(workspace, {
    managed: () => "New terminal in the worktree",
    folder: () => "New terminal in the folder",
  })
}

/** What the agent row's second line names: the project (resolved by the
 * caller, which has the project list) or the folder, home-collapsed.
 *
 * Returned as a tagged value rather than a bare string so the row can pick the
 * right glyph without re-deriving which kind of agent it is. */
export type AgentLocation =
  { kind: "project"; projectId: string } | { kind: "folder"; label: string }

export function workspaceLocation(
  workspace: AgentWorkspaceWire,
): AgentLocation {
  return matchWorkspace<AgentLocation>(workspace, {
    managed: (w) => ({ kind: "project", projectId: w.project_id }),
    folder: (w) => ({ kind: "folder", label: w.folder_label }),
  })
}

/** The name to show for an agent: its title when it has one, the branch it
 * tracks otherwise, and for a standalone agent its folder's own name. Creation
 * guarantees a standalone agent has a title, so that last fallback is belt and
 * braces rather than a path users reach.
 *
 * Twin of `AgentSession::display_label` in dux-core, pinned by shared vectors:
 * a twin that answers differently calls one agent two things. */
export function sessionLabel(session: {
  title: string | null
  workspace: AgentWorkspaceWire
}): string {
  if (session.title) {
    return session.title
  }
  return matchWorkspace(session.workspace, {
    managed: (w) => w.branch_name,
    folder: (w) => folderName(w.folder_path) ?? w.folder_path,
  })
}

/** The last NAMED component of a path, or null when it has none. Twin of Rust's
 * `Path::file_name`, which `display_label` uses, including the rules a naive
 * split-on-slash gets wrong:
 * - a trailing slash is ignored
 * - a trailing `.` is not a name (`/a/notes/.` is `notes`)
 * - a last component of `..` is no name at all, not the label `..`
 * - `/` and the empty string have none, and the caller falls back to the path */
export function folderName(folderPath: string): string | null {
  const named = folderPath.split("/").filter((s) => s !== "" && s !== ".")
  const last = named[named.length - 1]
  if (last === undefined || last === "..") return null
  return last
}

/** Whether an agent's current branch has drifted from the branch it was created
 * on, and what that original was. `drifted` is false for a standalone agent
 * because it has no branch to drift. Twin of dux-core's
 * `agent_tabs::branch_drifted`, pinned by shared vectors; keep the
 * empty-initial guard identical in both. */
export function branchDriftOf(workspace: AgentWorkspaceWire): {
  drifted: boolean
  initial: string
} {
  return matchWorkspace(workspace, {
    managed: (w) => ({
      drifted: !!w.initial_branch && w.initial_branch !== w.branch_name,
      initial: w.initial_branch,
    }),
    folder: () => ({ drifted: false, initial: "" }),
  })
}

// The sidebar names a standalone folder by its last component only, the full
// home-collapsed path being one glance away in the header chip and the info
// panel. Home is written `$HOME`, since a bare `~` is a symbol rather than a
// name; the root stays `/`.
export function folderDisplayName(label: string): string {
  const trimmed = label.replace(/\/+$/, "")
  if (label === "~" || trimmed === "~") return "$HOME"
  if (trimmed === "") return label === "" ? "" : "/"
  return trimmed.split("/").pop() || trimmed
}
