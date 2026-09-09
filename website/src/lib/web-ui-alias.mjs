import { createRequire } from "node:module"
import { fileURLToPath } from "node:url"

// The homepage figure imports the real React components out of
// `crates/dux-web/web/src`, and two things must hold for that to resolve the way
// it does inside the web app itself:
//
//   1. `@/…`, the web app's own alias for its `src` directory, which every one of
//      its components imports through.
//   2. One React, and specifically the WEB APP's copy. Its dependencies sit in
//      its own `node_modules` and resolve React from there by plain Node
//      resolution, which an alias declared here does not reach; two live copies
//      set the hook dispatcher on one and read it from the other, surfacing as
//      "cannot read properties of null (reading 'useContext')".
//
// The Astro build and the drift-guard test runner both read this, so the figure
// cannot render one way in the test and another on the site.

const here = (p) => fileURLToPath(new URL(p, import.meta.url))

export const WEB_UI_SRC = here("../../../crates/dux-web/web/src")
export const WEB_UI_MODULES = here("../../../crates/dux-web/web/node_modules")
// The app's project root. The dev server must be allowed to READ from here, not
// just resolve into it: the components pull real assets out of the app's
// dependencies, and Vite refuses to serve a file outside its root unless allowed.
export const WEB_UI_ROOT = here("../../../crates/dux-web/web")

/**
 * Every React import, this project's and the app's components' alike, points at
 * the APP's copy, which is what collapses them onto one React.
 *
 * Vite's dev SSR runner does not externalize an import matching an alias, so
 * aliasing these CommonJS packages straight to files makes Vite inline them as
 * ESM (`module is not defined`). The bridge modules below are ESM facades using
 * `createRequire` instead, which also preserves the singleton: externalized
 * app-side packages use Node resolution and reach these same files.
 */
const BRIDGE_ROOT = here("./web-ui-react-bridge")

const reactEntries = new Map([
  [BRIDGE_ROOT + "/react.mjs", WEB_UI_MODULES + "/react/index.js"],
  [BRIDGE_ROOT + "/react-jsx-runtime.mjs", WEB_UI_MODULES + "/react/jsx-runtime.js"],
  [BRIDGE_ROOT + "/react-jsx-dev-runtime.mjs", WEB_UI_MODULES + "/react/jsx-dev-runtime.js"],
  [BRIDGE_ROOT + "/react-dom.mjs", WEB_UI_MODULES + "/react-dom/index.js"],
  [BRIDGE_ROOT + "/react-dom-client.mjs", WEB_UI_MODULES + "/react-dom/client.js"],
  [BRIDGE_ROOT + "/react-dom-server.mjs", WEB_UI_MODULES + "/react-dom/server.js"],
])

export function webUiAlias() {
  // Put subpaths before package names: Vite aliases also match `find/…`, so a
  // bare `react` entry placed first would swallow `react/jsx-runtime`.
  return [
    { find: "@", replacement: WEB_UI_SRC },
    { find: "react/jsx-runtime", replacement: BRIDGE_ROOT + "/react-jsx-runtime.mjs" },
    { find: "react/jsx-dev-runtime", replacement: BRIDGE_ROOT + "/react-jsx-dev-runtime.mjs" },
    { find: "react-dom/client", replacement: BRIDGE_ROOT + "/react-dom-client.mjs" },
    { find: "react-dom/server", replacement: BRIDGE_ROOT + "/react-dom-server.mjs" },
    { find: "react", replacement: BRIDGE_ROOT + "/react.mjs" },
    { find: "react-dom", replacement: BRIDGE_ROOT + "/react-dom.mjs" },
  ]
}

/**
 * Turn the site-owned bridge modules into SSR-safe ESM facades around the app's
 * CommonJS React entry points. Named exports are derived from the installed
 * entry point, so this stays in lockstep with React upgrades, and `createRequire`
 * plus every target path are embedded so Node caches the same real files for the
 * facade and for app-side packages.
 */
export function webUiReactBridge() {
  const require = createRequire(import.meta.url)

  return {
    name: "web-ui-react-bridge",
    enforce: "pre",
    transform(_code, id, options) {
      const target = reactEntries.get(id)
      if (!target || !options?.ssr) return

      const names = Object.keys(require(target)).filter(
        (name) => name !== "default" && /^[$A-Z_a-z][$\w]*$/.test(name),
      )

      return [
        'import { createRequire } from "node:module"',
        `const value = createRequire(import.meta.url)(${JSON.stringify(target)})`,
        "export default value",
        ...names.map((name) => `export const ${name} = value.${name}`),
      ].join("\n")
    },
  }
}
