// HTTP client for agent provider-tab lifecycle (create / close / retarget),
// mirroring `terminalsApi.ts`. Tab byte I/O rides the dedicated per-tab PTY
// socket (`tabPtyUrl` in `lib/ptySocket.ts`); the lifecycle rides these scoped
// REST verbs. Like the other REST clients: `credentials: "same-origin"`, a JSON
// body where one is needed, and the per-connection id stamped as
// `X-Connection-Id` so the server can scope operation toasts back to this client.
// A non-2xx is thrown as a typed `TabsApiError` carrying the HTTP status + parsed
// message; the caller surfaces it as a sonner toast.

import { getConnectionId } from "./connection"

// A failed tabs REST call. `status` is the HTTP status (0 for a network/transport
// failure with no response); `message` is the parsed server detail.
export class TabsApiError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "TabsApiError"
    this.status = status
  }
}

// The 201 body for a tab create: the new tab's id (used to open the nested PTY
// socket and focus it) plus the resolved provider.
export interface CreatedTab {
  tab_id: string
  provider: string
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
    throw new TabsApiError("Could not reach the server.", 0)
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new TabsApiError(detail || `request failed (${resp.status})`, resp.status)
  }
  // 204 No Content (Support close) and empty bodies have nothing to parse.
  if (resp.status === 204) return undefined as T
  const text = await resp.text().catch(() => "")
  if (!text) return undefined as T
  try {
    return JSON.parse(text) as T
  } catch {
    return undefined as T
  }
}

export const tabsApi = {
  // Create a Support tab. `provider` omitted → the server uses the project
  // default. Returns the new tab id + resolved provider.
  create: (sessionId: string, provider?: string) =>
    request<CreatedTab>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs`,
      provider === undefined ? {} : { provider },
    ),
  // Close a tab. For the Main tab (`tabId === sessionId`) the server DETACHES the
  // agent (200, session survives); for a Support tab it destroys the tab (204).
  remove: (sessionId: string, tabId: string) =>
    request<void>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs/${encodeURIComponent(tabId)}`,
    ),
  // Retarget a tab's provider (effective on its next launch).
  patch: (sessionId: string, tabId: string, provider: string) =>
    request<void>(
      "PATCH",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs/${encodeURIComponent(tabId)}`,
      { provider },
    ),
}
