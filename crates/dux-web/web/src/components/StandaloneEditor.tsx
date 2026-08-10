import { FileCode2, Loader2 } from "lucide-react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { EditorBody } from "@/components/EditorBody"
import { useIsMobile } from "@/hooks/use-mobile"
import { useVisualViewportHeight } from "@/hooks/use-visual-viewport"
import { swallowMissedFileDrop } from "@/lib/editorDrop"
import { useDux } from "@/lib/store"
import { keyboardLikelyOpen } from "@/lib/viewport"

// The standalone editor surface: a whole browser tab that is nothing but the
// editor, at `#/editor/agent/<sid>[/<mode>/<encoded-path>]`. It is a full
// second SPA instance (bootstrap, spine, events socket, restart reload — that
// cost is accepted); this shell only composes the extracted `EditorBody`
// full-viewport under a minimal header naming the agent. There is
// deliberately NO in-app way out (no "Open in dux" link): this surface is a
// browser tab the user opened, and the browser's own controls — Back, or
// closing the tab — are the exit. The store still listens for hash changes,
// so any hash navigation (a bookmark, a manually edited address) keeps
// swapping surfaces through the URL as before.
//
// Phones reach this surface deliberately (best-effort, a settled decision:
// Monaco is poor on touch, but editing on the go beats no editor at all), so
// the shell applies the same two mobile-root behaviors `MobileApp` has:
// env(safe-area-inset-*) padding, and pinning the root to the visual
// viewport height so Monaco and its toolbar are never hidden behind the soft
// keyboard. The explorer starts collapsed there (EditorBody's `standalone`
// prop carries that).
export function StandaloneEditorShell() {
  const { editorTarget, routeNotFound, spine } = useDux()
  const isMobile = useIsMobile()
  // Unlike MobileApp (which only pins the terminal screen), the editor IS
  // the whole surface and always holds a focusable text input, so the pin
  // applies whenever the API reports a height on a phone.
  const viewportHeight = useVisualViewportHeight()
  const constrainToKeyboard = isMobile && viewportHeight !== null
  // Same rule as MobileApp: keep the bottom inset for the home indicator,
  // except while the shell is pinned above an open keyboard, where the inset
  // would leave a dead strip between the editor and the keys.
  const dropBottomInset =
    constrainToKeyboard &&
    viewportHeight !== null &&
    keyboardLikelyOpen(viewportHeight, window.innerHeight)

  const sessionId = editorTarget?.sessionId ?? null
  const session =
    sessionId !== null
      ? spine?.sessions.find((s) => s.id === sessionId)
      : undefined
  const agentName =
    session !== undefined ? session.title || session.branch_name : sessionId

  return (
    <div
      className="flex min-h-0 flex-col overflow-hidden bg-background"
      // The same floor the overlay has: a file dropped anywhere but the tree's
      // own rows would otherwise navigate this tab to the file and discard
      // every unsaved buffer. See `swallowMissedFileDrop`.
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
            <FileCode2 className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate text-sm font-medium">
              {agentName}
            </span>
          </div>
          <EditorBody
            key={editorTarget.sessionId}
            sessionId={editorTarget.sessionId}
            standalone
          />
        </>
      )}
    </div>
  )
}
