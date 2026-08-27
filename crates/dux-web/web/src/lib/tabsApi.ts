// Scoped REST client for provider-tab lifecycle. PTY bytes use the dedicated
// socket; these requests carry the connection id for status routing and surface
// failures as `TabsApiError`.

import { createJsonRequest } from "./jsonRequest"

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

// The 200 body for a tab close: whether that close detached the agent (it was
// the LAST live tab). Both the session-slot branch and the extra-tab branch of
// `DELETE .../tabs/:tab` return this shape, so the caller never has to guess the
// outcome from a stale local snapshot. `undefined` only for an older server that
// still replies with a bodiless 204.
export interface ClosedTab {
  detached: boolean
}

const request = createJsonRequest(
  (message, status) => new TabsApiError(message, status),
  { mapSerializationErrors: true },
)

export const tabsApi = {
  // Create an extra tab. `provider` omitted → the server uses the project
  // default. Returns the new tab id + resolved provider.
  create: (sessionId: string, provider?: string) =>
    request<CreatedTab>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs`,
      provider === undefined ? {} : { provider },
    ),
  // Close a tab. For the session-slot tab (`tabId === sessionId`) this stops that
  // tab (detaching the agent only if it was the last live tab); for an extra tab
  // it destroys the tab (same detach-if-last-live rule). Either way the 200 body
  // carries the authoritative `{ detached }` outcome — the caller should use it
  // rather than guessing from a pre-close snapshot.
  remove: (sessionId: string, tabId: string) =>
    request<ClosedTab | undefined>(
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
  // Remember the tab the user just focused on this agent (a dedicated verb,
  // matching the one-route-per-action style of the other tab REST calls).
  // `tabId` of `null` clears the memory. Fire-and-forget: this is a
  // high-frequency, user-paced write with no status/toast on the server side,
  // so a failure here is logged and otherwise swallowed rather than
  // surfaced to the user or allowed to block the (already-applied) local
  // selection change.
  setFocusedTab: (sessionId: string, tabId: string | null) =>
    request<void>(
      "PUT",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/focused-tab`,
      { tab_id: tabId },
    ).catch((err: unknown) => {
      console.error("Failed to persist the focused tab.", err)
    }),
}
