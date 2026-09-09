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

// The 200 body for a tab close: whether the close detached the agent (it was the last live
// tab), and which tab took the session slot when the closed tab held it. `promoted` is absent
// for an ordinary extra tab, and the spine has not caught up when this resolves.
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
  // Close a tab. The agent detaches when it was the last live one, and closing the slot tab
  // promotes the next tab in strip order; the 200 body carries both outcomes. The agent's
  // only tab is refused with a 400, because an agent always has a slot.
  remove: (sessionId: string, tabId: string) =>
    request<ClosedTab | undefined>(
      "DELETE",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/tabs/${encodeURIComponent(tabId)}`,
    ),
  // Start a dormant tab. It is the only start that gets past a recorded launch failure, which
  // opening the tab's PTY socket refuses; dispatching the launch clears that verdict, so the
  // pane behind the retiring card attaches to a launch already in flight.
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
  // Remember the tab the user just focused; `tabId` of `null` clears it. Fire-and-forget: a
  // high-frequency write whose failure is logged rather than allowed to block the
  // already-applied local selection change.
  setFocusedTab: (sessionId: string, tabId: string | null) =>
    request<void>(
      "PUT",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/focused-tab`,
      { tab_id: tabId },
    ).catch((err: unknown) => {
      console.error("Failed to persist the focused tab.", err)
    }),
}
