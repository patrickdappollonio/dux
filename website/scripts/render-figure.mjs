// Render the web-UI figure to a file: a `prep` step producing an artifact the
// site reads, run before `astro dev` and `astro build` alike.
//
// It renders here rather than inside the Astro page because the components and
// the store seed have to share one module graph: from inside a page they land in
// different instances and the tree renders without its workspace.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

import { webUiAlias, webUiReactBridge } from "../src/lib/web-ui-alias.mjs";

const APP = fileURLToPath(new URL("../../crates/dux-web/web/", import.meta.url));
const OUT = fileURLToPath(new URL("../src/figure/figure.html", import.meta.url));

// A fresh clone has this repository's source but not the sibling app's
// dependencies, so they are installed here rather than in a setup step a workflow
// file or a contributing guide can forget. `npm ci` is idempotent, so the common
// case costs a spawn. Install only, never `npm run build`: the figure renders
// from source, so building the app would produce output nothing reads.
if (!existsSync(APP + "node_modules/react/index.js")) {
  if (!existsSync(APP + "package.json")) {
    console.error(
      "render-figure: crates/dux-web/web is missing from this checkout, so the " +
        "figure cannot be rendered. This should not happen in a clone.",
    );
    process.exit(1);
  }

  console.log("render-figure: installing the dux web app's dependencies (first run only)...");
  const lockfile = existsSync(APP + "package-lock.json");
  const install = spawnSync("npm", [lockfile ? "ci" : "install", "--no-audit", "--no-fund"], {
    cwd: APP,
    stdio: "inherit",
  });
  if (install.status !== 0) {
    console.error(
      "render-figure: installing the dux web app's dependencies failed. Run " +
        "`cd crates/dux-web/web && npm ci` to see why.",
    );
    process.exit(1);
  }
}

const server = await createServer({
  configFile: false,
  root: fileURLToPath(new URL("..", import.meta.url)),
  logLevel: "silent",
  appType: "custom",
  server: { middlewareMode: true, hmr: false },
  plugins: [webUiReactBridge()],
  resolve: { alias: webUiAlias() },
  oxc: { jsx: { runtime: "automatic" } },
});

try {
  const react = await server.ssrLoadModule("react");
  const { renderToStaticMarkup } = await server.ssrLoadModule("react-dom/server");
  const { seedFigureWorkspace } = await server.ssrLoadModule("/src/figure/seed.ts");
  const { WebUIFigure } = await server.ssrLoadModule("/src/figure/WebUIFigure.tsx");

  // Seed before rendering: the components read the store, so it has to hold the
  // fabricated workspace by the time React is asked for markup.
  seedFigureWorkspace();
  const html = renderToStaticMarkup(react.createElement(WebUIFigure));

  // A render far smaller than a real one means the tree collapsed or the seed
  // did not land. Refuse to overwrite a good artifact with that.
  const MIN_BYTES = 20_000;
  if (html.length < MIN_BYTES) {
    console.error(
      `render-figure: rendered only ${html.length} bytes, under the ${MIN_BYTES} floor. ` +
        "The components rendered but the workspace probably did not reach them.",
    );
    process.exit(1);
  }

  await writeFile(OUT, html);
  console.log(`render-figure: wrote src/figure/figure.html (${html.length} bytes).`);
} finally {
  await server.close();
}
