// Scoped REST client for project mutations. Requests carry the connection id so
// server-side operation status routes back to the caller; failures are `ProjectsApiError`.

import { createJsonRequest } from "./jsonRequest"
import type { DeleteWorktreeReply } from "./worktreeDelete"
import type {
  AcceptedOperation,
  BranchWarningView,
  InspectKind,
  ProjectView,
  ProjectWorktreeEntryView,
  StartupLogContent,
  StartupLogsList,
} from "./types"

// A failed projects REST call. `status` is the HTTP status (0 for a network/
// transport failure with no response); `message` is the parsed server detail.
export class ProjectsApiError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "ProjectsApiError"
    this.status = status
  }
}

// PATCH body for a project's settings. Each scalar is tri-state: omit to leave untouched,
// `null` to clear to the inherited default, a value to set it. `env` replaces wholesale.
export interface PatchProjectBody {
  provider?: string | null
  auto_reopen_agents?: boolean | null
  startup_command?: string | null
  env?: Record<string, string>
}

const request = createJsonRequest(
  (message, status) => new ProjectsApiError(message, status),
)

export const projectsApi = {
  // 201 with the created project once it surfaces, otherwise 202 with the
  // operation id to correlate the outcome on the events socket.
  create: (body: {
    path: string
    name?: string
    checkout_default?: boolean
    // Birth an empty initial commit so an unborn repo can back worktrees. The
    // backend no-ops when the repo already has commits.
    create_initial_commit?: boolean
    // Adopt a plain folder: `git init`, seed a starter .gitignore, empty initial
    // commit, then register. Outranks `create_initial_commit` server-side.
    init_repo?: boolean
  }) => request<ProjectView | AcceptedOperation>("POST", "/api/v1/projects", body),
  remove: (id: string) =>
    request<void>("DELETE", `/api/v1/projects/${encodeURIComponent(id)}`),
  // The destructive cascade: removes the project, its agents and their worktrees
  // from disk, where the plain `remove` above keeps the worktrees.
  deleteWithWorktrees: (id: string) =>
    request<void>(
      "DELETE",
      `/api/v1/projects/${encodeURIComponent(id)}?delete_worktrees=true`,
    ),
  patch: (id: string, body: PatchProjectBody) =>
    request<void>("PATCH", `/api/v1/projects/${encodeURIComponent(id)}`, body),
  reorder: (projectIds: string[]) =>
    request<void>("POST", "/api/v1/projects/reorder", { project_ids: projectIds }),
  pull: (id: string) =>
    request<void>("POST", `/api/v1/projects/${encodeURIComponent(id)}/pull`),
  checkoutDefault: (id: string) =>
    request<void>("POST", `/api/v1/projects/${encodeURIComponent(id)}/checkout-default`),
  // List a project's managed worktrees for the "Attach worktree" picker.
  worktrees: (id: string) =>
    request<{ entries: ProjectWorktreeEntryView[] }>(
      "GET",
      `/api/v1/projects/${encodeURIComponent(id)}/worktrees`,
    ),
  // Remove one managed worktree from disk. The server re-validates: a path that is
  // not a managed worktree of this project is a 404, one an agent holds is a 409.
  // `deleteBranch` force-deletes the worktree's branch as well; absent means false.
  // The reply says what actually happened to the branch, since `git branch -D` can refuse.
  deleteWorktree: (id: string, worktreePath: string, deleteBranch: boolean) =>
    request<DeleteWorktreeReply>(
      "DELETE",
      `/api/v1/projects/${encodeURIComponent(id)}/worktrees?path=${encodeURIComponent(worktreePath)}&delete_branch=${deleteBranch}`,
    ),
  // Managed-worktree counts for every project, so the project picker can label its rows.
  worktreeCounts: () =>
    request<{ counts: Record<string, number> }>(
      "GET",
      "/api/v1/projects/worktree-counts",
    ),
  // Project-scoped startup-command logs: every run across every agent of the project,
  // newest first, newest contents pre-loaded. `sessionsApi.startupLogs` is agent-scoped.
  startupLogs: (id: string) =>
    request<StartupLogsList>(
      "GET",
      `/api/v1/projects/${encodeURIComponent(id)}/startup-logs`,
    ),
  // Read one project-scoped startup-command log file by name (empty name returns
  // the newest run in the project).
  startupLogContent: (id: string, name?: string) =>
    request<StartupLogContent>(
      "GET",
      `/api/v1/projects/${encodeURIComponent(id)}/startup-logs/content${
        name ? `?name=${encodeURIComponent(name)}` : ""
      }`,
    ),
  // Branch pre-flight for the add-project flow: inspect a candidate repo path and
  // report its current branch plus a non-default-branch warning.
  inspectPath: (path: string) =>
    request<{
      // Path classification. Absent from an older backend, which the store
      // treats as "repo".
      kind?: InspectKind
      // The enclosing repository root for `kind: "repo_subdir"`; null/absent
      // when inside git's internal directory (no user-facing root to name).
      repo_root?: string | null
      // For `kind: "plain"`: starter-.gitignore candidate directory names
      // found in the folder. Absent when empty.
      gitignore_candidates?: string[]
      current_branch: string | null
      warning: BranchWarningView | null
      // `false` for a freshly `git init`'d repo with no commits (unborn HEAD).
      // Absent from an older backend, which the store treats as "has commits".
      has_commits?: boolean
    }>("GET", `/api/v1/projects/inspect?path=${encodeURIComponent(path)}`),
}
