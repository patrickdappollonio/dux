// HTTP client for the config-mutating operations the palette / dialogs trigger:
// persist the global env map, replace the macro list wholesale, toggle the
// Changes-pane visibility flag, and reload config from disk. These used to ride
// the retired `/ws` `command` channel (`persist_global_env`, `update_macros`,
// `set_changes_pane_visible`, `reload_config`); since the Phase 6 cutover they
// are scoped REST verbs, stamping the per-connection id so the server can route
// each operation's status toast back to the initiating client.
//
// The server validates each request (e.g. macro names/text/surface) and persists
// to `config.toml`, emitting `config.changed` over `/ws/events` so every client
// refetches `GET /api/v1/bootstrap`. A non-2xx throws so the caller can toast it.

import { getConnectionId } from "./connection"
import type { MacroView } from "./types"

async function send(method: string, path: string, body: unknown): Promise<void> {
  const headers: Record<string, string> = { "content-type": "application/json" }
  const id = getConnectionId()
  if (id) headers["x-connection-id"] = id
  let resp: Response
  try {
    resp = await fetch(path, {
      method,
      credentials: "same-origin",
      headers,
      body: JSON.stringify(body),
    })
  } catch {
    throw new Error("Could not reach the server.")
  }
  if (!resp.ok) {
    const detail = (await resp.text().catch(() => "")).trim()
    throw new Error(detail || `request failed (${resp.status})`)
  }
}

export const configApi = {
  // Replace the entire `[macros]` map (the macro editor saves wholesale).
  updateMacros: (entries: MacroView[]) =>
    send("PUT", "/api/v1/macros", { entries }),
  // Persist the workspace-wide env map (replace-wholesale).
  persistGlobalEnv: (env: Record<string, string>) =>
    send("PUT", "/api/v1/global-env", { env }),
  // Persist the Changes-pane visibility flag (`config.ui.show_changes_pane`).
  setChangesPaneVisible: (visible: boolean) =>
    send("PUT", "/api/v1/ui/changes-pane", { visible }),
  // Reload config from disk (the "Reload config" palette command).
  reload: () => send("POST", "/api/v1/config/reload", {}),
  // Flip the random pet-name default. Parameterless: the server owns the value
  // and flips it, then emits `config.changed`.
  toggleRandomizedPetNameDefault: () =>
    send("POST", "/api/v1/defaults/toggle-randomized-pet-name", {}),
  // Swap the PR banner between the top and bottom of the agent pane.
  togglePrBannerPosition: () =>
    send("POST", "/api/v1/ui/toggle-pr-banner-position", {}),
  // Flip GitHub PR integration (and its engine-side PR-sync side effects).
  toggleGithubIntegration: () =>
    send("POST", "/api/v1/ui/toggle-github-integration", {}),
  // Read the raw config.toml text for the Monaco editor. Returns the file
  // verbatim (or the plain render of the running config if none exists yet).
  readRawConfig: async (): Promise<string> => {
    let resp: Response
    try {
      resp = await fetch("/api/v1/config/raw", { credentials: "same-origin" })
    } catch {
      throw new Error("Could not reach the server.")
    }
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).trim()
      throw new Error(detail || `request failed (${resp.status})`)
    }
    const body = (await resp.json()) as { content: string }
    return body.content
  },
  // Validate + write the raw config.toml text. A 400 (invalid TOML) throws with
  // the server's parse message so the editor can surface it inline.
  writeRawConfig: (content: string) =>
    send("PUT", "/api/v1/config/raw", { content }),
}
