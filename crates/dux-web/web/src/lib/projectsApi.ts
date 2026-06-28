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
  ProjectView,
  ProjectWorktreeEntryView,
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
  create: (body: { path: string; name?: string; checkout_default?: boolean }) =>
    request<ProjectView>("POST", "/api/v1/projects", body),
  remove: (id: string) =>
    request<void>("DELETE", `/api/v1/projects/${encodeURIComponent(id)}`),
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
  // Branch pre-flight for the add-project flow: inspect a candidate repo path and
  // report its current branch + a non-default-branch warning. Replaces the retired
  // `/ws` `inspect_project_path` request → `project_path_inspection` reply.
  inspectPath: (path: string) =>
    request<{
      current_branch: string | null
      warning: BranchWarningView | null
    }>("GET", `/api/v1/projects/inspect?path=${encodeURIComponent(path)}`),
}
