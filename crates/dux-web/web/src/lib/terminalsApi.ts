// Scoped REST client for companion-terminal lifecycle. PTY bytes use the
// dedicated socket; these requests carry the connection id for status routing
// and surface failures as `TerminalsApiError`.

import { createJsonRequest } from "./jsonRequest"

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

const request = createJsonRequest(
  (message, status) => new TerminalsApiError(message, status),
  { mapSerializationErrors: true },
)

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
