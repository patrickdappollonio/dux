import { Loader2 } from "lucide-react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { EditorBody } from "@/components/EditorBody"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { swallowMissedFileDrop } from "@/lib/editorDrop"
import { useDux } from "@/lib/store"
import { rootKey } from "@/lib/editorRoot"
import { keyboardLikelyOpen } from "@/lib/viewport"
import { SHELL_SAFE_TOP, shellHeight } from "@/lib/appViewport"

// The standalone editor surface: a whole browser tab that is nothing but the
// editor, at `#/editor/<root>[/<mode>/<encoded-path>]`. It is a full second SPA
// instance, and that cost is accepted. On a computer there is deliberately no
// in-app way out: the browser's own Back and tab close are the exit. A phone's
// editor header leads with the Back every phone screen has, which pushes the
// agent's Changes screen (see `leaveStandaloneEditor`), because a phone opens
// the editor in this same tab from there. Phones reach it best-effort, so the
// shell carries `MobileApp`'s two mobile-root behaviors, the safe-area
// padding and the visual-viewport pin that keeps Monaco clear of the keyboard.
export function StandaloneEditorShell() {
  const { editorTarget, routeNotFound } = useDux()
  const isMobile = useIsMobile()
  // The editor IS the whole surface and always holds a focusable input, so the
  // pin applies whenever the API reports a height on a phone.
  const viewportHeight = useVisualViewportHeight()
  const constrainToKeyboard = isMobile && viewportHeight !== null
  // Keep the bottom inset for the home indicator, except while pinned above an
  // open keyboard, where it leaves a dead strip between the editor and the keys.
  const dropBottomInset =
    constrainToKeyboard &&
    viewportHeight !== null &&
    keyboardLikelyOpen(viewportHeight, window.innerHeight)

  return (
    <div
      className="flex min-h-0 flex-col overflow-hidden bg-background"
      // A file dropped anywhere but the tree's own rows would navigate this tab to
      // it and discard every unsaved buffer. See `swallowMissedFileDrop`.
      onDragOver={swallowMissedFileDrop}
      onDrop={swallowMissedFileDrop}
      style={{
        height: shellHeight(
          constrainToKeyboard && viewportHeight !== null ? viewportHeight : null,
        ),
        paddingTop: SHELL_SAFE_TOP,
        paddingBottom: dropBottomInset ? 0 : "env(safe-area-inset-bottom)",
        paddingLeft: "env(safe-area-inset-left)",
        paddingRight: "env(safe-area-inset-right)",
      }}
    >
      {routeNotFound !== null ? (
        // The address names an agent this workspace does not have: the same
        // truthful screen the main app renders, filling the tab.
        <AgentNotFound sessionId={routeNotFound.sessionId} />
      ) : editorTarget === null ? (
        // Booting: the spine has not resolved the deep link yet.
        <div className="flex h-full items-center justify-center text-muted-foreground">
          <Loader2 className="size-5 motion-safe:animate-spin" />
        </div>
      ) : (
        // The body's own header names what this editor is rooted at.
        <EditorBody
          key={rootKey(editorTarget.root)}
          root={editorTarget.root}
          standalone
        />
      )}
    </div>
  )
}
