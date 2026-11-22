// @vitest-environment jsdom
// The `md:`/`max-md:` classes style for `data-layout` (see layoutVariants.test.ts),
// so the page entry must have set it before the app's first render, or that
// render is styled for no layout at all.
import { expect, it, vi } from "vitest"

let layoutAtRender: string | undefined | null = null
vi.mock("react-dom/client", () => ({
  createRoot: () => ({
    render: () => {
      layoutAtRender = document.documentElement.dataset.layout
    },
  }),
}))
vi.mock("./App.tsx", () => ({ default: () => null }))
vi.mock("./components/AuthGate.tsx", () => ({ AuthGate: () => null }))
vi.mock("./components/RootErrorBoundary.tsx", () => ({ RootErrorBoundary: () => null }))
vi.mock("./lib/clipboard.ts", () => ({ watchComposition: () => {} }))
vi.mock("./lib/openerPage.ts", () => ({ answerOpenedTabs: () => {} }))
vi.mock("./lib/sw.ts", () => ({ registerServiceWorker: () => {} }))

it("marks the layout on the document before the app first renders", async () => {
  document.body.innerHTML = '<div id="root"></div>'
  delete document.documentElement.dataset.layout
  await import("./main.tsx")
  expect(layoutAtRender).toBe("computer")
})
