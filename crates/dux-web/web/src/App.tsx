import type * as React from "react"

import { AddProjectDialog } from "@/components/AddProjectDialog"
import { StandaloneAgentDialog } from "@/components/StandaloneAgentDialog"
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
import { ConfirmVanishedEditorDialog } from "@/components/ConfirmVanishedEditorDialog"
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
import { TheaterChrome } from "@/components/TheaterChrome"
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
import { useTheaterEscape, useTheaterGesture } from "@/hooks/use-theater"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { useChangesPaneController } from "@/hooks/use-changes-pane-controller"
import {
  CHANGES_PANE_MIN_PERCENT,
  changesPaneVisible,
  TERMINAL_PANE_MIN_PERCENT,
  useDux,
} from "@/lib/store"
import { keyboardLikelyOpen } from "@/lib/viewport"

// All dialogs and the toaster, rendered ONCE by `App()` above whichever shell
// is active — desktop, mobile, or the standalone editor. Hoisted deliberately:
// the standalone surface needs the Toaster (save results), the OfflineOverlay,
// and `ConfirmCloseEditorTabDialog` (without which a dirty per-tab close there
// would be permanently inert) and `ConfirmVanishedEditorDialog` (raised by
// every surface with an open editor). Everything here portals to the body, so it
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
      <ConfirmVanishedEditorDialog />
      <GlobalEnvDialog />
      <MacrosDialog />
      <ProjectInfoDialog />
      <AgentInfoDialog />
      <ProjectSettingsDialog />
      <AgentStartupCommandDialog />
      <AgentEnvDialog />
      <StartupLogsDialog />
      <AddProjectDialog />
      <StandaloneAgentDialog />
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

// How many animation frames the Changes pane's re-show heal will wait for the
// panel library to publish a layout for the re-mounting panel before giving up
// and leaving the pane at whatever width it came back at. Measured in Chrome
// against react-resizable-panels 4.11.2: the answer arrives on the first frame,
// so this is headroom for a slow frame, not a budget anything relies on.
export { CHANGES_PANE_HEAL_FRAMES } from "@/hooks/use-changes-pane-controller"

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

  const { mountPercent, panelRef, onLayoutChange, onResize } =
    useChangesPaneController(showChanges)

  return (
    <SidebarProvider
      style={{ "--sidebar-width": sidebarWidth } as React.CSSProperties}
    >
      <AppSidebar />
      <SidebarInset className="flex h-svh min-h-0 flex-col overflow-hidden">
        {/* The pane header is the first of the two chrome stacks theater takes
            away. The other is the pull-request band plus the tab strip, inside
            TerminalArea; both collapse on the same flag and the ONE gesture
            above pays for the single refit between them. The sidebar and the
            Changes pane stay: theater is about the chrome stacked ON the pane,
            and both of those are already yours to put away. */}
        <TheaterChrome hidden={dux.theater}>
          <InsetHeader />
        </TheaterChrome>
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
            onLayoutChange={onLayoutChange}
          >
            {/* The terminal panel's defaultSize drops to 100% when the Changes
                panel is absent so it fills the width (no leftover sliver). The
                ids keep the two panels stable across the conditional mount.
                The controller remembers a user-dragged split and supplies it as
                the default when the Changes panel is mounted again.

                UNITS: never a bare number. react-resizable-panels v4 reads a
                bare number as PIXELS, so a bare `minSize={14}` is a
                fourteen-pixel floor rather than 14%, and the pane can be
                dragged down to a sliver and, being collapsible, snapped from
                there to nothing.
                Every size here is an explicit string with its unit spelled
                out; see the units note at the top of lib/editorLayout.ts. */}
            <ResizablePanel
              id="terminal-pane"
              defaultSize={
                showChanges ? `${100 - mountPercent}%` : "100%"
              }
              minSize={`${TERMINAL_PANE_MIN_PERCENT}%`}
            >
              <TerminalArea />
            </ResizablePanel>
            {showChanges ? (
              <>
                <ResizableHandle />
                <ResizablePanel
                  id="changes-pane"
                  panelRef={panelRef}
                  defaultSize={`${mountPercent}%`}
                  minSize={`${CHANGES_PANE_MIN_PERCENT}%`}
                  // Still collapsible: dragging the divider off the edge is a
                  // legitimate way to put the pane away, and it WRITES that
                  // intent. The preference and the split are separate
                  // variables, so a silent collapse leaves the pane at zero
                  // width while the preference still says "visible": no reopen
                  // button, the pane's own hide item inside the zero-width
                  // pane, stuck until a reload. Mapping the collapse onto the
                  // preference makes hidden-by-drag and hidden-by-menu one
                  // state with one way back. Same precedent as the sidebar,
                  // where dragging the edge past its threshold sets exactly the
                  // state the collapse button sets.
                  //
                  // The controller defers the write until the gesture ends; see
                  // `useChangesPaneController` and `changesPaneCollapseStep`.
                  collapsible
                  onResize={onResize}
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
  // Both mounted ABOVE the shell switch, so they are one instance for the whole
  // page rather than one per shell: the gesture must not restart because a
  // rotation swapped shells, and two Escape listeners would exit twice.
  useTheaterGesture()
  useTheaterEscape()
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
