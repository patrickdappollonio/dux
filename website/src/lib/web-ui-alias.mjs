import { createRequire } from "node:module"
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
// The app's project root. The dev server has to be allowed to READ from here,
// not just resolve into it: the components pull real assets out of the app's
// dependencies (the variable font among them), and Vite refuses to serve a file
// outside its own root unless the path is on the allow list.
export const WEB_UI_ROOT = here("../../../crates/dux-web/web")

/**
 * Every React import, from this project AND from the app's components, points at
 * the APP's copy. That is what collapses them onto one React: the app's UI
 * packages (lucide-react, @base-ui/react and friends) sit in the app's
 * node_modules with a React beside them, and anything that lets them resolve it
 * themselves produces a second copy and a null dispatcher on `useContext`.
 *
 * Vite's dev SSR runner deliberately does not externalize an import that
 * matches an alias. Aliasing these CommonJS packages straight to files made
 * Vite inline those files as if they were ESM (`module is not defined`). The
 * site-owned bridge modules below become ESM facades which use Node's
 * `createRequire` to load the app's packages natively. That also preserves the
 * singleton: externalized app-side UI packages use Node resolution and reach
 * these exact same files.
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
 * Turn the site-owned bridge modules into SSR-safe ESM facades around the
 * app's CommonJS React entry points. Client/build transforms read the bridge
 * files normally, where Vite's production CommonJS pipeline handles them.
 *
 * Named exports are derived from the installed entry point so this stays in
 * lockstep with React upgrades instead of maintaining a handwritten export
 * list. `createRequire` and every target path are embedded in the facade, so
 * Node caches the same real files for the facade and for app-side packages.
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
