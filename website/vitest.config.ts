import { defineConfig } from "vitest/config"

import { webUiAlias, webUiReactBridge } from "./src/lib/web-ui-alias.mjs"

// The site's own unit tests plus the web-UI figure's drift guard
// (`src/figure/figure.test.tsx`), which imports the REAL components out of
// `crates/dux-web/web/src`. Those need the same `@` alias and the same single
// React copy the Astro build gives them, so both configs read the alias map from
// one module rather than restating it.
export default defineConfig({
  plugins: [webUiReactBridge()],
  resolve: { alias: webUiAlias() },
  // Vite 8 transforms with oxc. The components and this file are `.tsx`, so the
  // automatic JSX runtime has to be stated or the JSX reaches the parser raw.
  oxc: { jsx: { runtime: "automatic" } },
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx,mjs}"],
  },
})
