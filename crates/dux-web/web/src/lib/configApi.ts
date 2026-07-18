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
      // A hung server must not wedge callers that await these writes (the
      // customize-webapp dialog disables its whole form while one is
      // pending): give up after 15s and surface the failure instead.
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
  // Flip GitHub PR integration AND its engine-side PR-sync side effects (arming
  // or disarming the background poll, clearing cached PR statuses). This is why
  // the `ui.github_integration` Preferences row routes here instead of through
  // the generic settings PATCH: that logic must not be forked. Parameterless:
  // the server owns the value and flips it, which is safe only because the row
  // is sent ONLY when it actually changed, so "changed" means "flip". See the
  // `writeTarget` doc in settingsDescriptors.ts.
  toggleGithubIntegration: () =>
    send("POST", "/api/v1/ui/toggle-github-integration", {}),
  // Persist the instance identity (browser tab title + favicon colour). Either
  // field may be omitted; the server validates the favicon against the curated
  // colour set, caps/normalizes the title, persists to config.toml, and emits
  // `config.changed` so every client re-applies title + favicon.
  setInstanceIdentity: (body: {
    title?: string
    favicon?: string
  }): Promise<void> =>
    send("POST", "/api/v1/config/instance-identity", body),
  // Persist an explicit patch of the Settings modal's `[ui]`/`[capabilities]`/
  // `[defaults]` fields in one request. Groups and leaf fields are all
  // optional; an absent field is left untouched server-side. The server clamps
  // numeric fields to a documented ceiling and rejects an unrecognized enum
  // value (`pr_banner_position`, `defaults.provider`) with a 400 that leaves
  // config unchanged. `title`/`favicon` are NOT here, they stay on
  // `setInstanceIdentity`, and `ui.github_integration` keeps its own endpoint.
  //
  // AUTHORITY: `SettingsBody` in `crates/dux-web/src/config_routes.rs` decides
  // which keys are accepted. It is `deny_unknown_fields`, so a key invented
  // here comes back as a 400 rather than being silently dropped.
  //
  // The leaf type is deliberately `Record<string, SettingValue>` and NOT a
  // hand-listed key union. The only caller (`buildWrites` in
  // `CustomizeWebappDialog.tsx`) builds exactly that and passes it as a
  // variable, not an object literal: excess-property checking only fires on
  // literals, and an index signature declares no properties for an
  // all-optional target to compare against. A hand-listed union here therefore
  // matches everything and rejects nothing. The previous one omitted the whole
  // `defaults` group while that group shipped, and `tsc` never noticed, which
  // is worse than no type because it reads as a guard. The real cross-language
  // guard is a pair of loud tests: "the settings-PATCH key set matches the
  // server's accepted fields" in `settingsDescriptors.test.ts` and
  // `set_settings_accepts_every_key_the_modal_can_send` in `config_routes.rs`.
  patchSettings: (patch: {
    ui?: Record<string, SettingValue>
    capabilities?: Record<string, SettingValue>
    defaults?: Record<string, SettingValue>
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
