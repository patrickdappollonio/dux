import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"

import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { ChangedFiles } from "@/components/ChangedFiles"
import { InsetHeader } from "@/components/InsetHeader"
import { PrBanner } from "@/components/PrBanner"
import { AppSidebar } from "@/components/Sidebar"
import { SidebarProvider } from "@/components/ui/sidebar"

import { WebUIFigure } from "./WebUIFigure"
import { seedFigureWorkspace } from "./seed"
import { focusedSessionId, spine } from "./workspace"

// THE DRIFT GUARD.
//
// The homepage figure renders the web UI's REAL components to static HTML at
// build time. That coupling is the whole value of it and also its whole risk: a
// change in the app that makes one of these components need a browser (a
// `window` read during render, an eager import of something DOM-bound, a hook
// that has no server snapshot) would turn the homepage into a blank rectangle,
// and nobody editing `crates/dux-web/web` has any reason to look at the website.
//
// So it is enforced rather than trusted. Every component the figure imports is
// rendered here STANDALONE, under a plain Node environment with no jsdom, which
// is the same environment the Astro build renders it in. If one stops rendering
// there, this test fails, `npm test` fails, and the build that runs it fails
// loudly instead of shipping an empty figure.
//
// Keep this list equal to what `WebUIFigure.tsx` imports. Adding a component to
// the figure means adding a case here.

seedFigureWorkspace()

const session = spine.sessions.find((s) => s.id === focusedSessionId)!

// Each entry renders one real component on its own. `AppSidebar` and the tab
// strip both read the sidebar context, so those go inside the real provider,
// which is itself one of the imported pieces.
const CASES: { name: string; render: () => string }[] = [
  {
    name: "AppSidebar",
    render: () =>
      renderToStaticMarkup(
        <SidebarProvider open>
          <AppSidebar />
        </SidebarProvider>,
      ),
  },
  { name: "InsetHeader", render: () => renderToStaticMarkup(<InsetHeader />) },
  {
    name: "AgentTabsStrip",
    render: () =>
      renderToStaticMarkup(
        <SidebarProvider open>
          <AgentTabsStrip session={session} activeTabId={focusedSessionId} />
        </SidebarProvider>,
      ),
  },
  {
    name: "PrBanner",
    render: () => renderToStaticMarkup(<PrBanner pr={session.pr!} position="top" />),
  },
  { name: "ChangedFiles", render: () => renderToStaticMarkup(<ChangedFiles />) },
]

describe("web UI figure", () => {
  it.each(CASES)("$name renders to static HTML outside a browser", ({ render }) => {
    const html = render()
    expect(html.length).toBeGreaterThan(0)
    expect(html).toContain("<")
  })

  // The composed figure, which is what the homepage actually ships. Asserting on
  // the fabricated CONTENT rather than on markup shape: if the store seed ever
  // stops reaching the components, every one of these disappears while the
  // wrapper divs stay, and a length check alone would not notice.
  it("renders the whole figure with the fabricated workspace in it", () => {
    const html = renderToStaticMarkup(<WebUIFigure />)
    for (const needle of [
      "checkout-retry", // the focused agent
      "webhook-replay", // a sibling agent, from the sidebar's flat list
      "storefront", // a project
      "billing-api", // a second project
      "dux/checkout-retry", // the focused agent's branch, in the header
      "482", // the pull request on the PR lane
      "retry-policy.ts", // a staged file in the changes pane
      "CheckoutSummary.tsx", // an unstaged file in the changes pane
    ]) {
      expect(html).toContain(needle)
    }
  })

  // Zero client JavaScript is a hard requirement of the figure, and the way it
  // would break is a component reaching for a browser API during render and
  // someone "fixing" it with a client directive. A render that throws here says
  // so before that choice is ever available.
  it("needs no browser globals during render", () => {
    expect(typeof window).toBe("undefined")
    expect(typeof document).toBe("undefined")
    expect(() => renderToStaticMarkup(<WebUIFigure />)).not.toThrow()
  })
})
