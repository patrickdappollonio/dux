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

// The homepage's web-UI figure.
//
// This is NOT a mockup and NOT a screenshot. It is the dux web UI's own React
// components, imported straight out of `crates/dux-web/web/src` and rendered to
// static HTML while the site builds, against a fabricated workspace seeded into
// the real store (`seed.ts`). Astro renders a React component with no client
// directive at build time and ships ZERO JavaScript for it, so what reaches the
// visitor is markup and CSS: a photo made of the real thing rather than a photo
// of it. Nothing here can drift from the app, because there is no copy to drift.
//
// Three deliberate departures from `App.tsx`'s `DesktopShell`, all of them
// because a still frame has no runtime:
//
//   1. The terminal interior is `StaticTerminal`, a stylised block. xterm needs a
//      live DOM and a live byte stream. See that file for why this is permanent.
//   2. `ResizablePanelGroup` is replaced by a plain flex split at the same
//      proportions. The resizable panels size themselves from a measured
//      container, which a build-time render does not have, so they would emit
//      a collapsed layout.
//   3. `GlobalOverlays` (every dialog, the toaster, the offline modal) is
//      omitted. All of it renders nothing until opened, and it drags in the
//      editor's eager Monaco import, which cannot initialize off a browser.
//
// Everything else on screen is the shipped component doing its shipped job.
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
