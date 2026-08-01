import { fileURLToPath } from "node:url"

// The web UI figure on the homepage renders the REAL React components out of
// `crates/dux-web/web/src` to static HTML at build time. Two things have to be
// true for that import to resolve the way it does inside the web app itself:
//
//   1. `@/…`, the web app's own alias for its `src` directory. Every one of
//      its components imports through it, so without this nothing resolves.
//
//   2. ONE React, and specifically the WEB APP's copy. The web app has its own
//      `node_modules`, so its dependencies (`lucide-react`, `@base-ui/react`,
//      `sonner`) resolve React from there by plain Node resolution, which an
//      alias declared over here does not reach. Two live copies means React's
//      hook dispatcher is set on one and read from the other, which surfaces as
//      "cannot read properties of null (reading 'useContext')" the moment an
//      icon renders. MEASURED, not assumed: aliasing these at the site's own
//      copy (with `resolve.dedupe` and with every dependency inlined) still
//      failed exactly that way, and pointing them at the web app's copy is what
//      made the render succeed. So the arrow goes this direction.
//
// Both the Astro build (`astro.config.mjs`) and the drift-guard test runner
// (`vitest.config.ts`) read this, so the figure cannot render one way in the
// test and another way on the site.

const here = (p) => fileURLToPath(new URL(p, import.meta.url))

export const WEB_UI_SRC = here("../../../crates/dux-web/web/src")
export const WEB_UI_MODULES = here("../../../crates/dux-web/web/node_modules")

/**
 * Every React import, from this project AND from the app's components, points at
 * the APP's copy. That is what collapses them onto one React: the app's UI
 * packages (lucide-react, @base-ui/react and friends) sit in the app's
 * node_modules with a React beside them, and anything that lets them resolve it
 * themselves produces a second copy and a null dispatcher on `useContext`.
 *
 * KNOWN LIMITATION: `astro dev` cannot serve the figure. React is CommonJS, and
 * these paths are outside this project, so Vite's dev SSR runner treats them as
 * source, inlines them, evaluates them as ES modules, and throws
 * `module is not defined` at the first `module.exports`. `astro build` bundles,
 * so the built site is correct; `npm run build && npm run preview` is the way to
 * look at the figure.
 *
 * What was measured, so the next person does not repeat it. Aliasing React to
 * THIS project's copy instead: does not fix dev, fails identically. Adding
 * `ssr.external`: no change. `ssr.noExternal` over the app's tree: no change.
 * `resolve.dedupe`: no change. `ssr.optimizeDeps.include` DOES make a bare
 * `import "react"` load, which is the one thing that worked, but the render then
 * dies on `react-dom/server` because the subpath aliases point at concrete files
 * and the dep optimizer never sees them. Dropping the subpath aliases breaks
 * resolution a different way. The remaining lead is making the dep optimizer
 * cover the subpaths, or vendoring the figure's React imports behind a module
 * this project owns.
 */
export function webUiAlias() {
  return {
    "@": WEB_UI_SRC,
    react: WEB_UI_MODULES + "/react",
    "react/jsx-runtime": WEB_UI_MODULES + "/react/jsx-runtime.js",
    "react/jsx-dev-runtime": WEB_UI_MODULES + "/react/jsx-dev-runtime.js",
    "react-dom": WEB_UI_MODULES + "/react-dom",
    "react-dom/client": WEB_UI_MODULES + "/react-dom/client.js",
    "react-dom/server": WEB_UI_MODULES + "/react-dom/server.js",
  }
}
