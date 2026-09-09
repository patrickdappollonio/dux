// HTTP client for the stateless utility reads the add-project and new-agent dialogs need. A
// non-2xx throws, so the caller can surface a toast and clear its loading state.

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

// POST helper matching `get`'s error contract. No `X-Connection-Id` header: mkdir resolves
// synchronously over HTTP, so nothing rides the status stream.
async function post<T>(path: string, body: unknown): Promise<T> {
  let resp: Response
  try {
    resp = await fetch(path, {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    })
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
  // Browse a directory for the add-project picker. A null path starts at $HOME, and the reply
  // echoes the resolved `path` so the dialog can show where it landed.
  browse: (path: string | null) =>
    get<{ path: string; entries: DirEntryView[] }>(
      path === null
        ? "/api/v1/browse"
        : `/api/v1/browse?path=${encodeURIComponent(path)}`,
    ),
  // Create one new folder inside an existing parent. The server validates `name` to a single
  // visible path component; a non-2xx (409 exists, 400 invalid) throws the server message.
  mkdir: (parent: string, name: string) =>
    post<{ path: string }>("/api/v1/browse/mkdir", { parent, name }),
  // A freshly generated pet name for the new-agent dialog's "Use randomized pet name" preview.
  agentName: () => get<{ name: string }>("/api/v1/agent-name"),
}
