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
// the LAST live tab), and which tab took the session slot when the closed tab
// was the one holding it. The caller never has to guess either outcome from a
// stale local snapshot, which matters most for `promoted`: the spine that would
// answer it has not caught up when this resolves. `promoted` is absent for an
// ordinary extra tab's close.
export interface ClosedTab {
  detached: boolean
  promoted?: string
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
  // Close a tab: the tab is destroyed, and the agent detaches when it was the
  // last live one. Closing the session-slot tab promotes the next tab in strip
  // order into the slot. The 200 body carries both authoritative outcomes rather
  // than leaving the caller to guess from a pre-close snapshot. The agent's ONLY
  // tab is refused with a 400: an agent always has a slot.
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
