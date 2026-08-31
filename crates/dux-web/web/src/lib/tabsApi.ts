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
// the LAST live tab). Only an extra tab can be closed, and its close returns this
// shape, so the caller never has to guess the outcome from a stale local
// snapshot. `undefined` only for an older server that still replies with a
// bodiless 204.
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
  // Close an EXTRA tab: the tab is destroyed, and the agent detaches when it was
  // the last live one. The 200 body carries that authoritative `{ detached }`
  // outcome rather than leaving the caller to guess from a pre-close snapshot.
  // The agent's FIRST tab (the session-slot tab) cannot be closed and the route
  // refuses it with a 400, so nothing should send one here.
  remove: (sessionId: string, tabId: string) =>
    request<ClosedTab | undefined>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs/${encodeURIComponent(tabId)}`,
    ),
  // Start a DORMANT tab: the "Start session" press. It is the only start that
  // gets past a recorded launch failure, because opening the tab's PTY socket
  // deliberately refuses to launch a tab whose last run failed. Dispatching the
  // launch is itself what clears that verdict, so the pane that mounts behind
  // the retiring card attaches to a launch already in flight.
  start: (sessionId: string, tabId: string) =>
    request<void>(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs/${encodeURIComponent(tabId)}/start`,
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
