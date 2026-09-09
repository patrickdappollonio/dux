// HTTP client for the resource-monitor read behind the Task Manager. Stats are polled rather
// than pushed because the event bus names what changed and never carries the changed value.
// The server single-flights and caches for a second, so browsers polling in step cost one walk.

// One process inside a sampled tree.
export interface ProcessInfoView {
  name: string
  pid: number
  cpu_percent: number
  rss_bytes: number
  /** True for the entry that is the row's own root process. `children` includes the root so
   * the breakdown sums to the row's total; this marks the entry that restates the row. */
  is_root: boolean
}

// One sampled row. Mirrors `dux_core::viewmodel::ResourceStatsView`.
export interface ResourceStatsView {
  /** The spine id to join on: a tab id for `agent`, a terminal id for `terminal`. Null for
   * the `dux` and `total` rows, which describe no single entity. Join on this, never `label`. */
  id: string | null
  kind: "dux" | "agent" | "terminal" | "total"
  /** Human-readable description from core. Display only; never parse it. */
  label: string
  pid: number | null
  /** May exceed 100: a multi-threaded tree across cores legitimately does.
   * Never clamp it. */
  cpu_percent: number
  rss_bytes: number
  process_count: number
  children: ProcessInfoView[]
  /** Whether the breakdown says anything the row does not, and so whether to offer an expand
   * affordance. Read it; never re-derive from `children.length`, which is 1 for a leaf
   * because `children` includes the row's own root. Core owns the rule
   * (`ResourceStats::has_breakdown`) so this surface and the TUI cannot drift. */
  has_breakdown: boolean
}

export interface ResourcesResponse {
  rows: ResourceStatsView[]
}

// A failed resources fetch. `status` is the HTTP status, 0 for a transport failure with no
// response, so a caller can tell an engine restart (503) from a network blip.
export class ResourcesFetchError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = "ResourcesFetchError"
    this.status = status
  }
}

export const resourcesApi = {
  async get(signal?: AbortSignal): Promise<ResourcesResponse> {
    let resp: Response
    try {
      resp = await fetch("/api/v1/resources", {
        credentials: "same-origin",
        signal,
      })
    } catch (e) {
      // An aborted poll is not a failure to surface; rethrow so the caller can ignore it.
      if (e instanceof DOMException && e.name === "AbortError") throw e
      throw new ResourcesFetchError("Could not reach the server.", 0)
    }
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).trim()
      throw new ResourcesFetchError(
        detail || `request failed (${resp.status})`,
        resp.status,
      )
    }
    return (await resp.json()) as ResourcesResponse
  },
}
