import type * as React from "react"
import { useEffect, useRef } from "react"
import type { PanelImperativeHandle } from "react-resizable-panels"

import { AddProjectDialog } from "@/components/AddProjectDialog"
import { AgentEnvDialog } from "@/components/AgentEnvDialog"
import { AgentInfoDialog } from "@/components/AgentInfoDialog"
import { AgentStartupCommandDialog } from "@/components/AgentStartupCommandDialog"
import { AttachPullRequestDialog } from "@/components/AttachPullRequestDialog"
import { WorktreesDialog } from "@/components/WorktreesDialog"
import { AppSidebar } from "@/components/Sidebar"
import { StartupLogsDialog } from "@/components/StartupLogsDialog"
import { ChangedFiles } from "@/components/ChangedFiles"
import { ChangeProviderDialog } from "@/components/ChangeProviderDialog"
import { CommitDialog } from "@/components/CommitDialog"
import { EditorOverlay } from "@/components/EditorOverlay"
import { FirstLoadDialog } from "@/components/FirstLoadDialog"
import { ConfigEditorDialog } from "@/components/ConfigEditorDialog"
import { ConfirmDeleteTerminalDialog } from "@/components/ConfirmDeleteTerminalDialog"
import { ConfirmCloseTabDialog } from "@/components/ConfirmCloseTabDialog"
import { ConfirmForceReconnectDialog } from "@/components/ConfirmForceReconnectDialog"
import { ConfirmUseExistingBranchDialog } from "@/components/ConfirmUseExistingBranchDialog"
import { TaskManagerDialog } from "@/components/TaskManagerDialog"
import { ConfirmDiscardFileDialog } from "@/components/ConfirmDiscardFileDialog"
import { ConfirmCloseEditorTabDialog } from "@/components/ConfirmCloseEditorTabDialog"
import { CreateAgentDialog } from "@/components/CreateAgentDialog"
import { NewAgentPickerDialog } from "@/components/NewAgentPickerDialog"
import { RenameSessionDialog } from "@/components/RenameSessionDialog"
import { CheckoutDefaultBranchDialog } from "@/components/CheckoutDefaultBranchDialog"
import { DeleteProjectDialog } from "@/components/DeleteProjectDialog"
import { DeleteSessionDialog } from "@/components/DeleteSessionDialog"
import { GlobalEnvDialog } from "@/components/GlobalEnvDialog"
import { MacrosDialog } from "@/components/MacrosDialog"
import { MobileShell } from "@/components/MobileShell"
import { OfflineOverlay } from "@/components/OfflineOverlay"
import { StandaloneEditorShell } from "@/components/StandaloneEditor"
import { ProjectInfoDialog } from "@/components/ProjectInfoDialog"
import { ProjectSettingsDialog } from "@/components/ProjectSettingsDialog"
import { RemoveProjectDialog } from "@/components/RemoveProjectDialog"
import { CustomizeWebappDialog } from "@/components/CustomizeWebappDialog"
import { InsetHeader } from "@/components/InsetHeader"
import { TerminalArea } from "@/components/TerminalArea"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import {
  SidebarInset,
  SidebarProvider,
} from "@/components/ui/sidebar"
import { Toaster } from "@/components/ui/sonner"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import {
  CHANGES_PANE_COLLAPSE_EPSILON,
  CHANGES_PANE_DEFAULT_PERCENT,
  changesPaneVisible,
  collapseChangesPaneFromDrag,
  isChangesPaneDragCollapse,
  setChangesPanePercent,
  useDux,
} from "@/lib/store"
import { keyboardLikelyOpen } from "@/lib/viewport"

// All dialogs and the toaster, rendered ONCE by `App()` above whichever shell
// is active — desktop, mobile, or the standalone editor. Hoisted deliberately:
// the standalone surface needs the Toaster (save results), the OfflineOverlay,
// and `ConfirmCloseEditorTabDialog` (without which a dirty per-tab close there
// would be permanently inert). Everything here portals to the body, so it
// depends on no shell-specific provider. Shared JSX — never duplicated.
function GlobalOverlays() {
  return (
    <>
      <CommitDialog />
      <EditorOverlay />
      <CreateAgentDialog />
      <NewAgentPickerDialog />
      <RenameSessionDialog />
      <AttachPullRequestDialog />
      <ChangeProviderDialog />
      <DeleteSessionDialog />
      <ConfirmDeleteTerminalDialog />
      <ConfirmCloseTabDialog />
      <ConfirmForceReconnectDialog />
      <ConfirmUseExistingBranchDialog />
      <TaskManagerDialog />
      <ConfigEditorDialog />
      <CustomizeWebappDialog />
      <FirstLoadDialog />
      <ConfirmDiscardFileDialog />
      <ConfirmCloseEditorTabDialog />
      <GlobalEnvDialog />
      <MacrosDialog />
      <ProjectInfoDialog />
      <AgentInfoDialog />
      <ProjectSettingsDialog />
      <AgentStartupCommandDialog />
      <AgentEnvDialog />
      <StartupLogsDialog />
      <AddProjectDialog />
      <WorktreesDialog />
      <RemoveProjectDialog />
      <DeleteProjectDialog />
      <CheckoutDefaultBranchDialog />
      <Toaster />
      {/* The app-wide offline modal. Portals to the body and sits above every
          other surface, so DOM order here is irrelevant — keep it last. */}
      <OfflineOverlay />
    </>
  )
}

// Exported for the unit tests, which drive the panel callbacks directly; `App`
// below is still the only production caller.
export function DesktopShell() {
  const dux = useDux()
  const { sidebarWidth } = dux
  // The Changes pane honours config.ui.show_changes_pane (via the bootstrap
  // document). The runtime hide/show controls (the pane's ⋯ menu, the header's
  // show button, the Preferences row) all persist that same preference. When
  // hidden, the terminal panel takes the full width and the handle + panel are
  // unmounted (no leftover sliver).
  const showChanges = changesPaneVisible(dux)

  // The Changes panel's imperative handle, and the visibility it had on the
  // previous render. Both exist for the re-show below.
  const changesPanelRef = useRef<PanelImperativeHandle | null>(null)
  const wasShowingChangesRef = useRef(showChanges)

  // RE-SHOW AT A REAL WIDTH. The library caches this group's layout, keyed by
  // the joined panel ids, and prefers that cache over `defaultSize` when the
  // panels re-register (`mutableState.layouts[ids] ?? defaultLayout ??
  // computed`, react-resizable-panels 4.11.2). So a pane that left at zero
  // width comes BACK at zero, and nothing but a page reload used to clear it.
  // There is no API to drop the cache, so the pane is measured on the way back
  // in and resized if it returned as nothing.
  //
  // Only on the hidden→shown transition: the first mount has no cache to fight
  // (the panel is at its defaultSize), and a live drag must never be argued
  // with mid-gesture.
  useEffect(() => {
    const reshown = showChanges && !wasShowingChangesRef.current
    wasShowingChangesRef.current = showChanges
    if (!reshown) return
    const handle = changesPanelRef.current
    if (!handle) return
    if (handle.getSize().asPercentage >= CHANGES_PANE_COLLAPSE_EPSILON) return
    handle.resize(`${CHANGES_PANE_DEFAULT_PERCENT}%`)
    // The header's spacer mirrors this number, so it has to move with the
    // panel; the group's own layout report would follow, but not before the
    // next frame.
    setChangesPanePercent(CHANGES_PANE_DEFAULT_PERCENT)
  }, [showChanges])

  return (
    <SidebarProvider
      style={{ "--sidebar-width": sidebarWidth } as React.CSSProperties}
    >
      <AppSidebar />
      <SidebarInset className="flex h-svh min-h-0 flex-col overflow-hidden">
        <InsetHeader />
        <div className="min-h-0 flex-1">
          <ResizablePanelGroup
            orientation="horizontal"
            className="size-full"
            // The split is LIFTED into the store because InsetHeader, this
            // group's sibling directly above, spans exactly the same width and
            // has to park the Macros button on the terminal pane's right edge.
            // It mirrors this percentage as a spacer, so nothing has to measure
            // pixels and the alignment survives any zoom or window size.
            //
            // `onLayoutChange` rather than `onLayoutChanged`: the former fires
            // on every pointer move of a drag, which is what makes the button
            // track the divider live instead of snapping to it on release. The
            // setter drops a write that would not change the value, so the
            // per-move cadence costs a render only when the split actually
            // moved.
            onLayoutChange={(layout) =>
              setChangesPanePercent(
                layout["changes-pane"] ?? CHANGES_PANE_DEFAULT_PERCENT,
              )
            }
          >
            {/* The terminal panel's defaultSize drops to 100% when the Changes
                panel is absent so it fills the width (no leftover sliver). The
                ids keep the two panels stable across the conditional mount.
                Note: a user-dragged split is NOT yet persisted across hide/show
                (defaultSize only applies on mount).

                UNITS: never a bare number. react-resizable-panels v4 reads a
                bare number as PIXELS, so `minSize={14}` was a fourteen-PIXEL
                floor rather than 14%, and the pane could be dragged down to a
                sliver and, being collapsible, snapped from there to nothing.
                Every size here is an explicit string with its unit spelled
                out; see the units note at the top of lib/editorLayout.ts. */}
            <ResizablePanel
              id="terminal-pane"
              defaultSize={
                showChanges ? `${100 - CHANGES_PANE_DEFAULT_PERCENT}%` : "100%"
              }
              minSize="30%"
            >
              <TerminalArea />
            </ResizablePanel>
            {showChanges ? (
              <>
                <ResizableHandle />
                <ResizablePanel
                  id="changes-pane"
                  panelRef={changesPanelRef}
                  defaultSize={`${CHANGES_PANE_DEFAULT_PERCENT}%`}
                  minSize="14%"
                  // Still collapsible: dragging the divider off the edge is a
                  // legitimate way to put the pane away. What changed is that
                  // it now WRITES that intent. The preference and the split are
                  // separate variables, so a silent collapse left the pane at
                  // zero width while the preference still said "visible": the
                  // header's reopen button stayed away, the pane's own hide
                  // item was inside the zero-width pane, and the pane was
                  // stuck until a reload. Mapping the collapse onto the
                  // preference makes hidden-by-drag and hidden-by-menu one
                  // state with one way back. Same precedent as the sidebar,
                  // where dragging the edge past its threshold sets exactly the
                  // state the collapse button sets.
                  collapsible
                  onResize={(size, _id, prevSize) => {
                    if (
                      isChangesPaneDragCollapse(
                        size.asPercentage,
                        prevSize?.asPercentage,
                      )
                    ) {
                      collapseChangesPaneFromDrag()
                    }
                  }}
                >
                  <ChangedFiles />
                </ResizablePanel>
              </>
            ) : null}
          </ResizablePanelGroup>
        </div>
      </SidebarInset>
    </SidebarProvider>
  )
}

// Mobile gets the hub-&-spoke shell (no SidebarProvider — that's desktop-only
// chrome). The shell fills the column above the fixed-height status bar; the
// shared dialogs/palette/toaster mount in both layouts. Split out from `App` so
// the store/viewport subscriptions live here and never run on the desktop path.
function MobileApp() {
  const { mobileScreen } = useDux()
  // On the terminal screen the soft keyboard opens, and h-svh does NOT shrink
  // for it — so the bottom of the shell (accessory bar + status bar) would hide
  // behind the keyboard. The visual viewport DOES track the keyboard, so pin the
  // mobile root to it there. Other screens (home/changes) have no focused text
  // input, so the viewport equals h-svh and we keep the default class height.
  const viewportHeight = useVisualViewportHeight()
  const constrainToKeyboard =
    mobileScreen === "terminal" && viewportHeight !== null
  // Drop the bottom safe-area inset only when we're actually pinning the shell
  // above an open keyboard — i.e. the terminal screen (constrainToKeyboard) with
  // the keyboard up. iOS does NOT zero env(safe-area-inset-bottom) when the
  // keyboard is open, so keeping it there would leave a dead strip between the
  // status bar and the keyboard. Everywhere else (keyboard down, or the
  // home/changes screens where opening the palette keyboard doesn't pin the
  // shell) the inset must stay to clear the home indicator. The `&&`
  // short-circuit avoids reading window.innerHeight when there's no viewport.
  const dropBottomInset =
    constrainToKeyboard &&
    viewportHeight !== null &&
    keyboardLikelyOpen(viewportHeight, window.innerHeight)

  return (
    // Safe-area padding lives on this single mobile root so EVERY screen
    // (terminal/home/changes) clears the notch, home indicator, and rounded
    // corners. Top/side insets always apply; the bottom inset drops only above an
    // open keyboard.
    <div
      className="flex min-h-0 flex-col overflow-hidden"
      style={{
        height:
          constrainToKeyboard && viewportHeight !== null
            ? viewportHeight
            : "100svh",
        paddingTop: "env(safe-area-inset-top)",
        paddingBottom: dropBottomInset ? 0 : "env(safe-area-inset-bottom)",
        paddingLeft: "env(safe-area-inset-left)",
        paddingRight: "env(safe-area-inset-right)",
      }}
    >
      <div className="min-h-0 flex-1">
        <MobileShell />
      </div>
    </div>
  )
}

function App() {
  const { standaloneEditor } = useDux()
  const isMobile = useIsMobile()
  // The standalone editor is checked BEFORE isMobile, deliberately: phones
  // must reach it (it is their one editor surface, best-effort by decision),
  // and an isMobile-first ladder would never let them past the mobile shell.
  const shell = standaloneEditor ? (
    <StandaloneEditorShell />
  ) : isMobile ? (
    <MobileApp />
  ) : (
    <DesktopShell />
  )
  return (
    <>
      {shell}
      <GlobalOverlays />
    </>
  )
}

export default App
