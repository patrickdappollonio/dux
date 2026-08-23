// Where an agent lives, defined ONCE and switched on exhaustively.
//
// An agent's home is one of exactly two things: a working copy dux created and
// owns, or a folder the user already had and dux only visits. Every git field
// (project, branch, source branch, birth branch, provenance, worktree path)
// belongs to the first shape and does not exist in the second, so the server
// sends a TAGGED value rather than a flat record with empty strings in it. An
// empty string on the wire is a lie some screen eventually renders.
//
// This mirrors `lib/terminalOwner.ts` deliberately, down to the exhaustive
// matcher-plus-`assertNever` shape: it is the same guarantee for a second
// either/or. The Rust side is `dux_core::viewmodel::AgentWorkspaceView`, whose
// own decisions live on `dux_core::model::AgentWorkspace` as exhaustive
// matches.

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

/** A handler per workspace variant, mapped over the union's `kind`.
 *
 * The second half of the guarantee, and the reason it exists: a `switch` inside
 * a HELPER only protects the helper. A consumer whose BEHAVIOUR depends on
 * which kind of agent this is takes one of these object literals, so a missing
 * key is a compile error at the consumer rather than a silently-wrong branch. */
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

/** The managed payload, or `null` for a standalone agent.
 *
 * LOSSY ON PURPOSE, and only for the sites where "does this agent have a branch
 * at all" is the entire question: a menu deciding whether the fork entry
 * exists, a banner deciding whether to render. A site whose behaviour DIFFERS
 * per kind must use `matchWorkspace` instead, so a third kind is a compile
 * error there. */
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

/** Whether the branch-identity features exist for this agent: fork, pull
 * requests, push, pull, branch rename and display, provenance, the worktree
 * manager. They are about a branch dux manages, and a standalone agent has none
 * whatever its folder contains.
 *
 * This is NOT the question the changes panel asks: that one is FOLDER driven,
 * answered by `changesQuietReason` below, because a standalone agent pointed at
 * a repository's top level gets a real changes panel. */
export function supportsBranchGit(workspace: AgentWorkspaceWire): boolean {
  return managedWorkspace(workspace) !== null
}

/** Why the changes region is quiet, or `null` when it is not.
 *
 * This doubles as "does the changes panel work here": a null reason IS the
 * working case. There is deliberately no separate boolean predicate; the one
 * that existed had no caller, and a second spelling of the same question is how
 * the two answers drift. */
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
 * tracks otherwise, and for a standalone agent its folder's own name.
 *
 * The twin of `AgentSession::display_label` in dux-core, and it exists for the
 * same reason: label sites fall back through the branch name, a standalone
 * agent has none, and without this rule they would render a nameless row.
 * Creation guarantees a standalone agent has a title, so the folder
 * fallback here is belt and braces rather than a path users reach.
 *
 * The folder half is pinned by shared vectors with `display_label`'s own test,
 * because a twin that answers differently is worse than no twin: the same agent
 * would be called two things on the two surfaces. */
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

/** The last NAMED component of a path, or null when it has none.
 *
 * The twin of Rust's `Path::file_name`, which is what `display_label` uses. The
 * rules a naive split-on-slash gets wrong, all MEASURED against the real
 * `Path::file_name`: a trailing slash is ignored, a trailing `.` is not a name
 * (`/a/notes/.` is `notes`), and a path whose last component is `..` has no name
 * at all rather than being labelled `..`. A path with no named component (`/`,
 * the empty string) answers null, and the caller falls back to the whole path,
 * exactly as the Rust side does. */
export function folderName(folderPath: string): string | null {
  const named = folderPath.split("/").filter((s) => s !== "" && s !== ".")
  const last = named[named.length - 1]
  if (last === undefined || last === "..") return null
  return last
}

/** Whether an agent's current branch has drifted from the branch it was created
 * on, and what that original was.
 *
 * `drifted` is false for a standalone agent, and not because the branches
 * happen to match: it has none, so there is nothing that could have drifted.
 * The TWIN of dux-core's `agent_tabs::branch_drifted`; pinned by shared
 * vectors. Keep the empty-initial guard identical in both. */
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
