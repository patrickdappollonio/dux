// Web handler map for the surface-aware command palette.
//
// The Rust `dux_core::palette` registry is the single source of truth for which
// commands exist and which surface (TUI | Web | Both) each appears on. The
// bootstrap document projects the Web/Both subset as `palette_commands` (id +
// description); this module supplies the per-id web handler. CommandPalette
// renders the "Commands" group from `bootstrap.palette_commands`, looking each
// id up here — entries with no handler are hidden with a dev warning.
//
// TWO-SIDED PIN: `PALETTE_HANDLERS`' keys must equal `EXPECTED_WEB_COMMANDS` in
// `paletteRegistry.test.ts`, which in turn is pinned to the Rust
// `web_surface_ids_match_expected_pin` test. Adding a handler here without
// updating both pins fails a gate.

import {
  openAddProject,
  openGlobalEnv,
  openKillRunning,
  openMacrosDialog,
  sortAgents,
  toggleChangesPane,
  toggleGithubIntegration,
  togglePrBannerPosition,
  toggleRandomizedPetNameDefault,
} from "@/lib/store"
import { configApi } from "@/lib/configApi"
import { toast } from "sonner"

// id (dashed core command name) -> action to run. Handlers perform the action
// only; CommandPalette closes the palette afterward.
export const PALETTE_HANDLERS: Record<string, () => void> = {
  "add-project": () => openAddProject(),
  "configure-global-env": () => openGlobalEnv(),
  "edit-macros": () => openMacrosDialog(),
  "kill-running": () => openKillRunning(),
  "reload-config": () => {
    configApi
      .reload()
      .catch((e) =>
        toast.error(
          e instanceof Error ? e.message : "Could not reload the config.",
        ),
      )
  },
  "sort-agents-by-created": () => sortAgents("created"),
  "sort-agents-by-name": () => sortAgents("name"),
  "sort-agents-by-updated": () => sortAgents("updated"),
  "toggle-github-integration": () => toggleGithubIntegration(),
  "toggle-pr-banner-position": () => togglePrBannerPosition(),
  "toggle-randomized-pet-name-default": () => toggleRandomizedPetNameDefault(),
  "toggle-remove-git-pane": () => toggleChangesPane(),
}
