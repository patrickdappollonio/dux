// HTTP client for the resource-monitor read behind the Task Manager.
//
// A plain GET (the read-only `changesApi.ts` pattern, with
// `credentials: "same-origin"`). Stats are polled rather than pushed on purpose:
// the event bus names what changed and never carries the changed value, and
// stats are a value that changes every sample. See `crates/dux-web/src/resource_routes.rs`.
//
// The server single-flights and caches for a second, so several browsers polling
// in step still cost one process walk.

// One process inside a sampled tree.
export interface ProcessInfoView {
  name: string
  pid: number
  cpu_percent: number
  rss_bytes: number
}

// One sampled row. Mirrors `dux_core::viewmodel::ResourceStatsView`.
export interface ResourceStatsView {
  /** The spine id to join on: a tab id for `agent`, a terminal id for
   * `terminal`. Null for the `dux` and `total` rows, which describe no single
   * spine entity. Join on this, NEVER on `label`. */
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
}

export interface ResourcesResponse {
  rows: ResourceStatsView[]
}

// A failed resources fetch, carrying the HTTP status (0 for a transport failure
// with no response) so the caller can distinguish "engine restarting" (503) from
// a network blip.
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
      // An aborted poll (the dialog closed mid-flight) is not a failure the
      // caller should surface; rethrow so it can be recognized and ignored.
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
