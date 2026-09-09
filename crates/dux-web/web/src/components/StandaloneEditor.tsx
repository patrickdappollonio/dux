import { FileCode2, Loader2, SquareTerminal } from "lucide-react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { EditorBody } from "@/components/EditorBody"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { swallowMissedFileDrop } from "@/lib/editorDrop"
import { useDux } from "@/lib/store"
import { rootKey } from "@/lib/editorRoot"
import { standaloneEditorName } from "@/lib/standaloneEditorName"
import { keyboardLikelyOpen } from "@/lib/viewport"

// The standalone editor surface: a whole browser tab that is nothing but the
// editor, at `#/editor/<root>[/<mode>/<encoded-path>]`. It is a full second SPA
// instance, and that cost is accepted. There is deliberately no in-app way out:
// the browser's own Back and tab close are the exit. Phones reach it best-effort,
// so the shell carries `MobileApp`'s two mobile-root behaviors, the safe-area
// padding and the visual-viewport pin that keeps Monaco clear of the keyboard.
export function StandaloneEditorShell() {
  const { editorTarget, routeNotFound, spine } = useDux()
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

  // The header's identity, from the same facts the sidebar row is drawn from,
  // so the tab and the row cannot disagree about what this editor is on.
  const named = standaloneEditorName(editorTarget?.root ?? null, spine)

  return (
    <div
      className="flex min-h-0 flex-col overflow-hidden bg-background"
      // A file dropped anywhere but the tree's own rows would navigate this tab to
      // it and discard every unsaved buffer. See `swallowMissedFileDrop`.
      onDragOver={swallowMissedFileDrop}
      onDrop={swallowMissedFileDrop}
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
        <>
          <div className="flex shrink-0 items-center gap-2 border-b px-3 py-2">
            {/* The root's own glyph, so a terminal-rooted tab reads as one at a
                glance rather than as an agent whose name happens to be a path. */}
            {named.glyph === "terminal" ? (
              <SquareTerminal className="size-4 shrink-0 text-muted-foreground" />
            ) : (
              <FileCode2 className="size-4 shrink-0 text-muted-foreground" />
            )}
            <span className="min-w-0 flex-1 truncate text-sm font-medium">
              {named.name}
            </span>
            {named.detail !== null && (
              <span className="min-w-0 shrink truncate text-xs text-muted-foreground">
                {named.detail}
              </span>
            )}
          </div>
          <EditorBody
            key={rootKey(editorTarget.root)}
            root={editorTarget.root}
            standalone
          />
        </>
      )}
    </div>
  )
}
