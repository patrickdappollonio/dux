// HTTP client for mutating agent-session operations (create/fork/from-worktree/
// from-pr, delete, rename, change-provider, toggle auto-reopen, reconnect,
// reorder). These used to ride the
// fire-and-forget `/ws` `sendCommand` channel; they are now scoped, programmable
// REST verbs so the server can authorize each one and route its operation toasts
// back to the initiating client.
//
// Like `git.ts`, every request is `credentials: "same-origin"` with a JSON body
// and stamps the per-connection id as `X-Connection-Id` (every endpoint reads
// it) so the server can scope the busy/success/error toasts — which still arrive
// over `/ws` — back to this client. A non-2xx is thrown as a typed
// `SessionsApiError` carrying the HTTP status + the parsed server message; the
// caller surfaces it as a sonner toast (the legacy `/ws` CommandResult that used
// to report these failures no longer fires for them).

import { getConnectionId } from "./connection"
import type {
  SessionView,
  StartupLogContent,
  StartupLogsList,
} from "./types"

// A failed sessions REST call. `status` is the HTTP status (0 for a network/
// transport failure with no response); `message` is the parsed server detail.
export class SessionsApiError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "SessionsApiError"
    this.status = status
  }
}

// The discriminated create body the server matches on `kind`.
export type CreateSessionBody =
  | { kind: "new"; project_id: string; name?: string }
  | { kind: "fork"; session_id: string; name?: string }
  | { kind: "from_worktree"; project_id: string; worktree_path: string; name?: string }
  | { kind: "from_pr"; project_id: string; pr: string; name?: string }

// PATCH body for a session. Every field is optional; an omitted field is left
// untouched. Setting `provider` triggers a pending reconnect server-side.
export interface PatchSessionBody {
  title?: string
  provider?: string
  auto_reopen?: boolean
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = {}
  // Every sessions endpoint reads the connection id to scope its toasts back to
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
    throw new SessionsApiError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new SessionsApiError(detail || `request failed (${resp.status})`, resp.status)
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

export const sessionsApi = {
  create: (body: CreateSessionBody) =>
    request<SessionView>("POST", "/api/v1/sessions", body),
  remove: (id: string, deleteWorktree: boolean) =>
    request<void>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(id)}?delete_worktree=${deleteWorktree}`,
    ),
  patch: (id: string, body: PatchSessionBody) =>
    request<{ provider_change?: string }>(
      "PATCH",
      `/api/v1/sessions/${encodeURIComponent(id)}`,
      body,
    ),
  reconnect: (id: string, force: boolean) =>
    request<void>("POST", `/api/v1/sessions/${encodeURIComponent(id)}/reconnect`, {
      force,
    }),
  // Force-kill the agent's running PTY (it detaches; it is NOT deleted). Used by
  // the kill-running modal. A non-2xx throws.
  kill: (id: string) =>
    request<void>("POST", `/api/v1/sessions/${encodeURIComponent(id)}/kill`),
  reorder: (projectId: string, sessionIds: string[]) =>
    request<void>("POST", "/api/v1/sessions/reorder", {
      project_id: projectId,
      session_ids: sessionIds,
    }),
  // Re-run the agent's project startup command in its worktree. The server runs
  // it off-thread and routes the busy/success/error toasts back over `/ws`, so
  // this resolves as soon as the run is accepted (a non-2xx still throws).
  rerunStartupCommand: (id: string) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(id)}/rerun-startup-command`,
    ),
  // List the agent's startup-command log files (newest first) with the newest
  // file's contents pre-loaded for immediate display.
  startupLogs: (id: string) =>
    request<StartupLogsList>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/startup-logs`,
    ),
  // Read one startup-command log file by name (empty name returns the newest).
  startupLogContent: (id: string, name?: string) =>
    request<StartupLogContent>(
      "GET",
      `/api/v1/sessions/${encodeURIComponent(id)}/startup-logs/content${
        name ? `?name=${encodeURIComponent(name)}` : ""
      }`,
    ),
}
