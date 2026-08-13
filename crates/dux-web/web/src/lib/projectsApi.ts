// HTTP client for mutating project operations (add — with an optional
// check-out-default-branch-first variant, remove, update per-project settings,
// reorder, refresh source checkout, check out the default branch). These used to
// ride the fire-and-forget `/ws` `sendCommand` channel; they are now scoped,
// programmable REST verbs so the server can authorize each one and route its
// operation toasts back to the initiating client.
//
// Like `git.ts` and `sessionsApi.ts`, every request is `credentials:
// "same-origin"` with a JSON body and stamps the per-connection id as
// `X-Connection-Id` (every endpoint reads it) so the server can scope the
// busy/success/error toasts — which still arrive over `/ws` — back to this
// client. A non-2xx is thrown as a typed `ProjectsApiError` carrying the HTTP
// status + the parsed server message; the caller surfaces it as a sonner toast.

import { getConnectionId } from "./connection"
import type {
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

// PATCH body for a project's settings. Each scalar is tri-state: omit the key to
// leave it untouched, send `null` to clear it back to the inherited default, or
// send a value to set it. `env` is replace-wholesale (omit = untouched).
export interface PatchProjectBody {
  provider?: string | null
  auto_reopen_agents?: boolean | null
  startup_command?: string | null
  env?: Record<string, string>
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = {}
  // Every projects endpoint reads the connection id to scope its toasts back to
  // this client. Omitted only while the `connected` frame has not set it yet.
  const id = getConnectionId()
  if (id) headers["x-connection-id"] = id
  let payload: string | undefined
  if (body !== undefined) {
    headers["content-type"] = "application/json"
    payload = JSON.stringify(body)
  }
  let resp: Response
  try {
    resp = await fetch(path, {
      method,
      credentials: "same-origin",
      headers,
      body: payload,
    })
  } catch {
    throw new ProjectsApiError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new ProjectsApiError(detail || `request failed (${resp.status})`, resp.status)
  }
  // 204 No Content (delete) and empty bodies have nothing to parse.
  if (resp.status === 204) return undefined as T
  const text = await resp.text().catch(() => "")
  if (!text) return undefined as T
  try {
    return JSON.parse(text) as T
  } catch {
    return undefined as T
  }
}

export const projectsApi = {
  create: (body: {
    path: string
    name?: string
    checkout_default?: boolean
    // Birth an empty initial commit before registering an unborn (`git init`,
    // no commits) repo so it can back worktrees. Backend no-ops if the repo
    // already has commits.
    create_initial_commit?: boolean
    // Adopt a plain (non-repo) folder: run `git init`, seed a starter
    // .gitignore, create an empty initial commit, then register. Outranks
    // `create_initial_commit` server-side (init subsumes the commit).
    init_repo?: boolean
  }) => request<ProjectView>("POST", "/api/v1/projects", body),
  remove: (id: string) =>
    request<void>("DELETE", `/api/v1/projects/${encodeURIComponent(id)}`),
  // The destructive cascade: `?delete_worktrees=true` routes the same DELETE to
  // `WireCommand::DeleteProject`, which removes the project, its agents, AND
  // their worktrees from disk (the plain `remove` above keeps the worktrees).
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
  // List a project's managed worktrees for the "Attach worktree" picker. Replaces
  // the retired `/ws` `list_project_worktrees` request → `project_worktrees` reply.
  worktrees: (id: string) =>
    request<{ entries: ProjectWorktreeEntryView[] }>(
      "GET",
      `/api/v1/projects/${encodeURIComponent(id)}/worktrees`,
    ),
  // Remove ONE managed worktree from disk. The server re-validates against a
  // fresh classification: a path that is not a managed worktree of this project
  // is a 404 and one an agent holds is a 409, so the UI's rules are not the only
  // thing standing between a stale list and a destroyed worktree.
  // `deleteBranch` force-deletes the branch the worktree is on as well. The
  // server defaults it to false when the parameter is absent, so a request that
  // says nothing never deletes a branch; the confirmation dialog is what decides
  // to ask, and it only asks when the worktree actually has a branch.
  deleteWorktree: (id: string, worktreePath: string, deleteBranch: boolean) =>
    request<void>(
      "DELETE",
      `/api/v1/projects/${encodeURIComponent(id)}/worktrees?path=${encodeURIComponent(worktreePath)}&delete_branch=${deleteBranch}`,
    ),
  // Managed-worktree counts for every project, so the project picker can label
  // its rows and an empty project is a choice rather than a surprise.
  worktreeCounts: () =>
    request<{ counts: Record<string, number> }>(
      "GET",
      "/api/v1/projects/worktree-counts",
    ),
  // List the PROJECT-scoped startup-command log files: every run across every
  // agent of the project, newest first, with the newest file's contents
  // pre-loaded. The agent-scoped counterpart is `sessionsApi.startupLogs`; both
  // return the same `StartupLogsList` shape, which is what lets one dialog serve
  // both scopes.
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
  // report its current branch + a non-default-branch warning. Replaces the retired
  // `/ws` `inspect_project_path` request → `project_path_inspection` reply.
  inspectPath: (path: string) =>
    request<{
      // Path classification. Optional: an older backend omits it (version
      // skew), and the store treats a missing kind as "repo" (mirroring the
      // `has_commits !== false` skew handling below).
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
      // Optional: an older backend that predates this field omits it (version
      // skew), and the store treats a missing value as "has commits".
      has_commits?: boolean
    }>("GET", `/api/v1/projects/inspect?path=${encodeURIComponent(path)}`),
}
