// HTTP client for companion-terminal lifecycle (create/delete), Phase 5 of the
// REST-first migration. These used to ride the fire-and-forget `/ws`
// `create_terminal`/`delete_terminal` commands; live terminal byte I/O now rides
// the dedicated PTY socket (`lib/ptySocket.ts`) and the lifecycle rides these
// scoped REST verbs.
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

async function request<T>(method: string, path: string): Promise<T> {
  const headers: Record<string, string> = {}
  // Scope any resulting status toasts back to this client. Omitted only while the
  // `connected` frame has not set the id yet.
  const id = getConnectionId()
  if (id) headers["x-connection-id"] = id
  let resp: Response
  try {
    resp = await fetch(path, {
      method,
      credentials: "same-origin",
      headers,
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
}
