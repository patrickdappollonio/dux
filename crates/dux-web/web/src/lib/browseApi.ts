// HTTP client for the two stateless "utility" reads the add-project / new-agent
// dialogs need. These used to ride the retired `/ws` request/reply pairs
// (`browse_dir` → `dir_entries`, `generate_agent_name` → `agent_name`); since the
// Phase 6 cutover they are plain GETs, matching the REST resource map in the
// rest-first architecture design.
//
// `credentials: "same-origin"` like the other read clients. A non-2xx throws so
// the caller can surface a toast and clear its loading state.

import type { DirEntryView } from "./types"

async function get<T>(path: string): Promise<T> {
  let resp: Response
  try {
    resp = await fetch(path, { credentials: "same-origin" })
  } catch {
    throw new Error("Could not reach the server.")
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new Error(detail || `request failed (${resp.status})`)
  }
  return (await resp.json()) as T
}

export const browseApi = {
  // Browse a directory for the add-project picker. A null path starts at $HOME
  // (the server resolves it). The reply echoes the resolved `path` so the dialog
  // can show where it landed plus the child `entries`.
  browse: (path: string | null) =>
    get<{ path: string; entries: DirEntryView[] }>(
      path === null
        ? "/api/v1/browse"
        : `/api/v1/browse?path=${encodeURIComponent(path)}`,
    ),
  // A freshly generated pet name for the new-agent dialog's "Use randomized pet
  // name" preview. Replaces the retired `/ws` `generate_agent_name` request.
  agentName: () => get<{ name: string }>("/api/v1/agent-name"),
}
