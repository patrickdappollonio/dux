// HTTP client for companion-terminal lifecycle (create/delete). Live terminal
// byte I/O rides the dedicated PTY socket (`lib/ptySocket.ts`); lifecycle rides
// these scoped REST verbs.
//
// Mirrors `sessionsApi.ts`: `credentials: "same-origin"`, JSON body, and the
// per-connection id stamped as `X-Connection-Id` so the server can scope any
// operation toasts (which still arrive over `/ws`) back to this client. A non-2xx
// is thrown as a typed `TerminalsApiError` carrying the HTTP status + parsed
// message; the caller surfaces it as a sonner toast.

import { getConnectionId } from "./connection"

// A failed terminals REST call. `status` is the HTTP status (0 for a network/
// transport failure with no response); `message` is the parsed server detail.
export class TerminalsApiError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "TerminalsApiError"
    this.status = status
  }
}

// The 201 body for a terminal create: the new terminal's id (used to open the
// nested PTY socket and focus it) plus its display label.
export interface CreatedTerminal {
  terminal_id: string
  label: string
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = {}
  // Scope any resulting status toasts back to this client. Omitted only while the
  // `connected` frame has not set the id yet.
  const id = getConnectionId()
  if (id) headers["x-connection-id"] = id
  if (body !== undefined) headers["content-type"] = "application/json"
  let resp: Response
  try {
    resp = await fetch(path, {
      method,
      credentials: "same-origin",
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    })
  } catch {
    throw new TerminalsApiError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new TerminalsApiError(
      detail || `request failed (${resp.status})`,
      resp.status,
    )
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

export const terminalsApi = {
  create: (sessionId: string) =>
    request<CreatedTerminal>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/terminals`,
    ),
  remove: (sessionId: string, terminalId: string) =>
    request<void>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/terminals/${encodeURIComponent(terminalId)}`,
    ),
  // Project terminals (a plain shell at the project's repo root with no agent
  // attached) ride the project-nested twins of the session routes.
  createForProject: (projectId: string) =>
    request<CreatedTerminal>(
      "POST",
      `/api/v1/projects/${encodeURIComponent(projectId)}/terminals`,
    ),
  removeForProject: (projectId: string, terminalId: string) =>
    request<void>(
      "DELETE",
      `/api/v1/projects/${encodeURIComponent(projectId)}/terminals/${encodeURIComponent(terminalId)}`,
    ),
  // A STANDALONE terminal (a plain shell in the user's home directory, owned by
  // neither an agent nor a project) rides UN-NESTED addresses, because there is
  // no owner to nest under and nothing that has to exist before it can be
  // created. The create takes no id at all.
  createStandalone: () =>
    request<CreatedTerminal>("POST", "/api/v1/terminals"),
  removeStandalone: (terminalId: string) =>
    request<void>("DELETE", `/api/v1/terminals/${encodeURIComponent(terminalId)}`),
  // Reorder the flat Terminals section as one global list (mirrors
  // `sessionsApi.reorderGlobal`). `terminalIds` must be the COMPLETE set of every
  // current terminal id (any owner), in the desired order; the server validates it
  // as a strict permutation and rejects a partial/stale set. Terminal order is
  // runtime-only (no SQLite), so this resets to creation order on restart.
  reorder: (terminalIds: string[]) =>
    request<void>("POST", "/api/v1/terminals/reorder", {
      terminal_ids: terminalIds,
    }),
}
