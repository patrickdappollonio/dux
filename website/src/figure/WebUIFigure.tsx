import type * as React from "react"

import { ChangedFiles } from "@/components/ChangedFiles"
import { InsetHeader } from "@/components/InsetHeader"
import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { PrBanner } from "@/components/PrBanner"
import { AppSidebar } from "@/components/Sidebar"
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar"
import { useDux } from "@/lib/store"

import { StaticTerminal } from "./StaticTerminal"
import { focusedSessionId } from "./workspace"

// The homepage's web-UI figure: the dux web UI's own React components, imported
// out of `crates/dux-web/web/src` and rendered to static HTML at build time
// against a fabricated workspace seeded into the real store (`seed.ts`). No
// client directive anywhere, so it ships zero JavaScript.
//
// It departs from `App.tsx`'s `DesktopShell` only where a still frame has no
// runtime, and each departure is permanent:
//
//   1. The terminal interior is `StaticTerminal`: xterm needs a live DOM and a
//      live byte stream.
//   2. `ResizablePanelGroup` becomes a plain flex split at the same proportions:
//      the resizable panels size themselves from a measured container and would
//      otherwise emit a collapsed layout.
//   3. `GlobalOverlays` is omitted: it renders nothing until opened and drags in
//      the editor's eager Monaco import, which cannot initialize off a browser.
export function WebUIFigure() {
  const { spine, sidebarWidth } = useDux()
  const session = spine?.sessions.find((s) => s.id === focusedSessionId)

  return (
    <SidebarProvider
      style={{ "--sidebar-width": sidebarWidth } as React.CSSProperties}
      // The figure is a fixed still: no state changes, so the provider is told
      // the sidebar is open rather than left to manage it.
      open
    >
      <AppSidebar />
      {/* `h-svh` matches `App.tsx` exactly: the inset is the app's full-height
          column, and the figure's document is sized to the iframe, so the frame
          IS the viewport. */}
      <SidebarInset className="flex h-svh min-h-0 flex-col overflow-hidden">
        <InsetHeader />
        <div className="flex min-h-0 flex-1">
          <div className="flex min-h-0 flex-[74] flex-col overflow-hidden">
            {session?.pr ? <PrBanner pr={session.pr} position="top" /> : null}
            {session ? (
              <AgentTabsStrip
                session={session}
                activeTabId={focusedSessionId}
              />
            ) : null}
            <div className="min-h-0 flex-1 overflow-hidden">
              <StaticTerminal />
            </div>
          </div>
          <div className="min-h-0 flex-[26] overflow-hidden border-l">
            <ChangedFiles />
          </div>
        </div>
      </SidebarInset>
    </SidebarProvider>
  )
}
