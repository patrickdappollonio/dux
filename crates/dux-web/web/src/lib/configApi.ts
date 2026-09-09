// HTTP client for the config-mutating operations the palette and dialogs trigger.
// Each request carries the per-connection id so the server can route its status
// toast back to the initiating client, and a non-2xx throws.
//
// The server validates and persists to `config.toml`, then emits `config.changed`
// so every client refetches its bootstrap document.

import { getConnectionId } from "./connection"
import type { SettingValue } from "./settingsDescriptors"
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
      // A hung server must not wedge callers that await these writes: a dialog
      // disables its whole form while one is pending.
      signal: AbortSignal.timeout(15_000),
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
  // Persist the flat agent-list sort mode (`config.ui.agent_sort`). The server
  // validates the value and rejects unknown modes.
  setAgentSort: (sort: string) =>
    send("POST", "/api/v1/ui/agent-sort", { sort }),
  // Reload config from disk (the app menu's "Reload config").
  reload: () => send("POST", "/api/v1/config/reload", {}),
  // Flips GitHub PR integration and its engine-side sync side effects together, so
  // the Preferences row routes here rather than forking that logic into the generic
  // settings PATCH. Parameterless: the server owns the value, which is safe only
  // because the row is sent only when it changed.
  toggleGithubIntegration: () =>
    send("POST", "/api/v1/ui/toggle-github-integration", {}),
  // Asks `gh` again right now: neither a preference nor a config write, but the way
  // back from a failure that has since passed, for someone who cannot restart dux
  // without taking every running agent with it. A changed answer refetches every
  // client's bootstrap document.
  recheckGithub: () => send("POST", "/api/v1/github/recheck", {}),
  // Saves `[server] tailscale` and, when a listener is up, moves the Tailscale
  // listener to match; bespoke because that second half is a live act only the serve
  // loop can perform.
  //
  // The reply is the sentence the server composed, which the TUI shows too, so a
  // second copy written here is how the two drift apart.
  setTailscaleMode: async (
    mode: string,
  ): Promise<{ mode: string; warning: boolean; message: string }> => {
    const headers: Record<string, string> = {
      "content-type": "application/json",
    }
    const id = getConnectionId()
    if (id) headers["x-connection-id"] = id
    let resp: Response
    try {
      resp = await fetch("/api/v1/server/tailscale-mode", {
        method: "POST",
        credentials: "same-origin",
        headers,
        body: JSON.stringify({ mode }),
        // Above the server's own bound on the change (it runs one bounded
        // address detection), so a slow answer is still an answer.
        signal: AbortSignal.timeout(15_000),
      })
    } catch {
      throw new Error("Could not reach the server.")
    }
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).trim()
      throw new Error(detail || `request failed (${resp.status})`)
    }
    return resp.json()
  },
  // Persists the instance identity: tab title and favicon colour, either omissible.
  // The server validates the favicon against the curated colour set and caps the title.
  setInstanceIdentity: (body: {
    title?: string
    favicon?: string
  }): Promise<void> =>
    send("POST", "/api/v1/config/instance-identity", body),
  // Persists a patch of the Settings modal's `[ui]`/`[capabilities]`/`[defaults]`
  // fields in one request; every group and leaf is optional and an absent field is
  // left untouched. `title` and `favicon` stay on `setInstanceIdentity`, and
  // `ui.github_integration` keeps its own endpoint.
  //
  // `SettingsBody` in `crates/dux-web/src/config_routes.rs` decides which keys are
  // accepted and is `deny_unknown_fields`, so an invented key is a 400.
  //
  // The leaf type is deliberately an index signature rather than a key union: the
  // caller passes a variable, so excess-property checking never fires and a union
  // here would match everything and reject nothing. Tests on both sides of the wire
  // are what actually pin the key set.
  patchSettings: (patch: {
    ui?: Record<string, SettingValue>
    capabilities?: Record<string, SettingValue>
    defaults?: Record<string, SettingValue>
    // Not a settings field: asks the server to emit no info status for this request.
    // Honored only for a patch confined to the accessory-bar field, so it can never
    // silence another write, and errors still fail loudly.
    quiet?: boolean
  }): Promise<void> => send("PATCH", "/api/v1/config/settings", patch),
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
