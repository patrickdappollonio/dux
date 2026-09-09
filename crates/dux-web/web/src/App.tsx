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
  setSidebarOpen,
  TERMINAL_PANE_MIN_PERCENT,
  useDux,
} from "@/lib/store"
import { keyboardLikelyOpen } from "@/lib/viewport"

// Every dialog and the toaster, rendered once above whichever shell is active.
// The standalone editor is a shell too and needs these, so nothing here may move
// into the desktop or mobile tree; everything portals to the body and depends on
// no shell-specific provider.
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
      {/* Portals to the body and sits above every other surface, so DOM order
          here is irrelevant. */}
      <OfflineOverlay />
    </>
  )
}

// Re-exported for the unit tests, which import it alongside `DesktopShell`.
export { CHANGES_PANE_HEAL_FRAMES } from "@/hooks/use-changes-pane-controller"

// Exported for the unit tests, which drive the panel callbacks directly; `App`
// below is still the only production caller.
export function DesktopShell() {
  const dux = useDux()
  const { sidebarWidth, sidebarOpen, theater } = dux
  // Theater suppresses the pane without writing the preference every other
  // hide/show control persists: the mode is transient, and a write here would
  // hide the pane for good and for every client. Derived rather than stored, so
  // a preference changed while the mode is on is what the pane comes back to.
  const showChanges = changesPaneVisible(dux) && !theater

  const { mountPercent, panelRef, onLayoutChange, onResize } =
    useChangesPaneController(showChanges)

  return (
    <SidebarProvider
      // Controlled, so the store owns both the state and its persistence: the
      // primitive's internal state keeps flipping, and writing, while theater
      // has the panel unmounted, and the layout the user came from is then
      // different on the way out.
      open={sidebarOpen}
      onOpenChange={setSidebarOpen}
      style={{ "--sidebar-width": sidebarWidth } as React.CSSProperties}
    >
      {/* Unmounted rather than collapsed: collapsed is the icon rail, which is
          chrome too. The swap is instant while the header stack below animates,
          because animating a width owned by the sidebar primitive or by
          react-resizable-panels would hand the terminal a stream of intermediate
          widths to be measured at. It lands inside the same layout gesture as
          the chrome collapse (see `useTheaterGesture`), so the whole change
          costs one refit at the geometry it settles on. */}
      {theater ? null : <AppSidebar />}
      <SidebarInset className="flex h-svh min-h-0 flex-col overflow-hidden">
        {/* Theater takes this stack and the band-plus-strip inside TerminalArea
            on the same flag, so the gesture above pays for one refit between
            them. The hidden Changes pane's reopen control lives in this header
            and goes with it; the floating pill is the chrome theater leaves. */}
        <TheaterChrome hidden={theater}>
          <InsetHeader />
        </TheaterChrome>
        <div className="min-h-0 flex-1">
          <ResizablePanelGroup
            orientation="horizontal"
            className="size-full"
            // The split lives in the store because InsetHeader, this group's
            // sibling above, mirrors the percentage as a spacer to park the
            // Macros button on the terminal pane's right edge, so nothing has to
            // measure pixels. `onLayoutChange` rather than `onLayoutChanged`: it
            // fires on every pointer move of a drag, so the button tracks the
            // divider live instead of snapping to it on release.
            onLayoutChange={onLayoutChange}
          >
            {/* The ids keep both panels stable across the conditional mount, and
                the controller supplies a remembered user-dragged split as the
                default when the Changes panel is mounted again.

                Units: never a bare number. react-resizable-panels v4 reads a
                bare number as pixels, so `minSize={14}` is a fourteen-pixel
                floor rather than 14%, and the pane can be dragged down to a
                sliver and, being collapsible, snapped from there to nothing.
                See the units note at the top of lib/editorLayout.ts. */}
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
                  // Collapsible, and the collapse writes the hide preference:
                  // the preference and the split are separate variables, so a
                  // silent collapse leaves a zero-width pane the preference
                  // still calls visible, with no reopen button and no way back
                  // until a reload. The write is deferred to the end of the
                  // gesture; see `changesPaneCollapseStep`.
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

// The hub-and-spoke phone shell, which has no SidebarProvider (desktop-only
// chrome). Split out of `App` so its store and viewport subscriptions never run
// on the desktop path.
function MobileApp() {
  const { mobileScreen } = useDux()
  // h-svh does not shrink for the soft keyboard but the visual viewport does,
  // so the terminal screen, the one with a focused text input, is pinned to the
  // viewport height; every other screen keeps the default class height.
  const viewportHeight = useVisualViewportHeight()
  const constrainToKeyboard =
    mobileScreen === "terminal" && viewportHeight !== null
  // iOS does not zero env(safe-area-inset-bottom) while the keyboard is open, so
  // a shell pinned above one would keep a dead strip between the status bar and
  // the keyboard. Everywhere else the inset stays to clear the home indicator.
  const dropBottomInset =
    constrainToKeyboard &&
    viewportHeight !== null &&
    keyboardLikelyOpen(viewportHeight, window.innerHeight)

  return (
    // Safe-area padding lives on this one mobile root so every screen clears the
    // notch, home indicator and rounded corners.
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
  // Above the shell switch, so a rotation that swaps shells neither restarts the
  // gesture nor leaves two Escape listeners exiting twice.
  useTheaterGesture()
  useTheaterEscape()
  // Checked before `isMobile` deliberately: phones must reach the standalone
  // editor, and an isMobile-first ladder never lets them past the mobile shell.
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
