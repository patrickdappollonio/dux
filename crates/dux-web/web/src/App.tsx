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
  changesPaneCollapseStep,
  setChangesPanePercent,
  useDux,
} from "@/lib/store"
import { keyboardLikelyOpen } from "@/lib/viewport"

// All dialogs and the toaster, rendered ONCE by `App()` above whichever shell
// is active — desktop, mobile, or the standalone editor. Hoisted deliberately:
// the standalone surface needs the Toaster (save results), the OfflineOverlay,
// and `ConfirmCloseEditorTabDialog` (without which a dirty per-tab close there
// would be permanently inert) and `ConfirmVanishedEditorDialog` (which only
// that surface ever raises). Everything here portals to the body, so it
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
export const CHANGES_PANE_HEAL_FRAMES = 5

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
  // Open from the hidden→shown transition until the heal below has had its
  // answer. The panel's layout reports are mount noise for that window; see
  // `changesPaneCollapseStep`.
  const reshowPendingRef = useRef(false)

  // RE-SHOW AT A REAL WIDTH. The library caches this group's layout, keyed by
  // the joined panel ids, and prefers that cache over `defaultSize` when the
  // panels re-register (`mutableState.layouts[ids] ?? defaultLayout ??
  // computed`, react-resizable-panels 4.11.2). So a pane that left at zero
  // width comes BACK at zero, and nothing but a page reload used to clear it.
  // There is no API to drop the cache, so the pane is measured on the way back
  // in and resized if it returned as nothing.
  //
  // WHY THIS WAITS FOR A FRAME. Every method on the panel's imperative handle
  // resolves the panel through the library's registry and THROWS when the entry
  // is missing (`Layout not found for Panel changes-pane`), rather than
  // returning nothing. Measured in Chrome against 4.11.2: at the moment this
  // effect runs, the entry is not there yet. The re-mounting Panel registers
  // itself in a layout effect and schedules the Group's re-registration through
  // a state update, and the Group is what publishes the layout; the parent's
  // passive effect lands in between. One animation frame later the call
  // succeeds. Calling it here unwound React out of an effect with no boundary
  // over it, and the whole screen went black on the first click of "Show
  // Changes pane", which is the bug this comment exists for.
  //
  // So the heal runs off animation frames and retries a bounded few times.
  // Deliberately NOT driven off the panel's own first `onResize` instead: a
  // ResizeObserver compares against a last-reported size that starts at zero,
  // so an element that mounts at 0x0 and stays there never reports at all, and
  // 0x0 is exactly the case the heal exists for. Every call is guarded: a
  // future quirk in the library must cost one more click on the reopen button,
  // never the screen.
  //
  // Only on the hidden→shown transition: the first mount has no cache to fight
  // (the panel is at its defaultSize), and a live drag must never be argued
  // with mid-gesture.
  useEffect(() => {
    const reshown = showChanges && !wasShowingChangesRef.current
    wasShowingChangesRef.current = showChanges
    if (!reshown) return
    reshowPendingRef.current = true
    let frames = 0
    let scheduled = requestAnimationFrame(function attempt() {
      const handle = changesPanelRef.current
      if (!handle) {
        reshowPendingRef.current = false
        return
      }
      let percent: number
      try {
        percent = handle.getSize().asPercentage
      } catch (err) {
        frames += 1
        if (frames < CHANGES_PANE_HEAL_FRAMES) {
          scheduled = requestAnimationFrame(attempt)
          return
        }
        reshowPendingRef.current = false
        console.warn(
          "[dux] the Changes panel never published a layout; it reopens at whatever width the panel library kept. Hide and show it again to retry.",
          err,
        )
        return
      }
      reshowPendingRef.current = false
      if (percent >= CHANGES_PANE_COLLAPSE_EPSILON) return
      try {
        handle.resize(`${CHANGES_PANE_DEFAULT_PERCENT}%`)
      } catch (err) {
        console.warn(
          "[dux] the Changes panel refused to resize back to its default width; it reopens at nothing. Hide and show it again to retry.",
          err,
        )
        return
      }
      // The header's spacer mirrors this number, so it has to move with the
      // panel; the group's own layout report would follow, but not before the
      // next frame.
      setChangesPanePercent(CHANGES_PANE_DEFAULT_PERCENT)
    })
    return () => {
      cancelAnimationFrame(scheduled)
      reshowPendingRef.current = false
    }
  }, [showChanges])

  // THE DRAG-COLLAPSE LATCH. `changesPaneCollapseStep` explains why the write
  // waits for the end of the gesture; these two refs are the state it reads.
  // Refs rather than state on purpose: both change on pointer-move cadence and
  // neither is rendered, so a re-render per report would be pure cost.
  const pointerDownRef = useRef(false)
  const collapseArmedRef = useRef(false)
  const collapseCommitRef = useRef<number | null>(null)

  // WRITE THE COLLAPSE FROM A TASK OF ITS OWN, never from inside the event that
  // produced it. Flipping the preference unmounts the panel and its separator,
  // and the library is not finished with them yet.
  //
  // Its `pointerup` handler runs on the DOCUMENT in the capture phase; this
  // shell's runs on the WINDOW in the capture phase, which is strictly earlier
  // in the same dispatch. So "the pointer is already up" did not mean the
  // library was out of its drag. It still ends the drag afterwards, and ending
  // it re-adds the group object it captured at pointerdown to the registry
  // (`hitRegions.forEach(({group}) => setGroupData(group, getGroupDataById(group.id)))`,
  // 4.11.2). That object was deleted when the panel unmounted, so re-adding it
  // resurrects a two-panel registration holding the one-panel layout, and
  // every registry read scans by group id and takes the FIRST match, so the
  // corpse shadows the live group from then on. Measured: after a drag-shut,
  // reopening left the pane an eleven-pixel sliver, the panel's layout never
  // appeared again, and the library threw `Invalid 2 panel layout: 100%` out of
  // its own ResizeObserver.
  //
  // A macrotask, not a microtask: microtask checkpoints run BETWEEN listeners
  // of the same dispatch, so a microtask would still land before the library's
  // handler. A timeout runs after the dispatch is over, with the panel still
  // mounted throughout it, which is all the library needs.
  const scheduleCollapseCommit = () => {
    if (collapseCommitRef.current !== null) return
    collapseCommitRef.current = window.setTimeout(() => {
      collapseCommitRef.current = null
      collapseChangesPaneFromDrag()
    }, 0)
  }
  useEffect(
    () => () => {
      if (collapseCommitRef.current !== null) {
        clearTimeout(collapseCommitRef.current)
      }
    },
    [],
  )

  // The gesture tracker. One persistent subscription rather than a listener
  // installed when the latch arms: the latch needs to know whether a pointer is
  // down BEFORE it decides to arm, so the tracker has to be listening already,
  // and a second one-shot listener would only add an ordering hazard against
  // this one clearing the flag.
  //
  // Capture phase, on `window`: a stranded latch is the original bug back again
  // (a zero-width pane whose preference still says visible), so the release
  // must not be droppable by anything calling stopPropagation on the way up.
  // Any pointer counts as a gesture; the panel only reports a collapse while
  // its own separator is being dragged, and a collapse that arrives with some
  // unrelated pointer held down is still committed, just on that pointer's
  // release.
  //
  // The commit runs while the panel is still mounted and the pointer is already
  // up, so the library is out of its drag by the time React unmounts anything.
  useEffect(() => {
    const onDown = () => {
      pointerDownRef.current = true
    }
    const onUp = () => {
      pointerDownRef.current = false
      if (!collapseArmedRef.current) return
      collapseArmedRef.current = false
      scheduleCollapseCommit()
    }
    window.addEventListener("pointerdown", onDown, true)
    window.addEventListener("pointerup", onUp, true)
    window.addEventListener("pointercancel", onUp, true)
    return () => {
      window.removeEventListener("pointerdown", onDown, true)
      window.removeEventListener("pointerup", onUp, true)
      window.removeEventListener("pointercancel", onUp, true)
    }
  }, [])

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
                  //
                  // The write is deferred to the end of the gesture; see the
                  // latch above and `changesPaneCollapseStep`.
                  collapsible
                  onResize={(size, _id, prevSize) => {
                    const step = changesPaneCollapseStep({
                      percent: size.asPercentage,
                      prevPercent: prevSize?.asPercentage,
                      pointerDown: pointerDownRef.current,
                      armed: collapseArmedRef.current,
                      reshowPending: reshowPendingRef.current,
                    })
                    if (step === "arm") {
                      collapseArmedRef.current = true
                    } else if (step === "disarm") {
                      collapseArmedRef.current = false
                    } else if (step === "commit") {
                      collapseArmedRef.current = false
                      scheduleCollapseCommit()
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
