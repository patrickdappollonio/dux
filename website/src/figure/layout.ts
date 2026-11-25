import { resolveShellLayout } from "@/lib/shellLayout"

// The figure shows the workspace as a visitor on a computer sees it. The app's
// `md:` and `max-md:` variants key on `data-layout` on <html>, which its runtime
// keeps current; a static page has no runtime, so the figure's page declares the
// answer the app's own resolver gives for a wide, fine-pointer screen.
export const FIGURE_LAYOUT = resolveShellLayout({
  width: 1440,
  screenShortSide: 900,
  coarse: false,
  choice: null,
})
