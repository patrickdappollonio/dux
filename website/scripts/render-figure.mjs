// Render the web-UI figure to a file, the same way `fetch-contributors.mjs`
// writes contributor data and `generate-webp.mjs` writes images: a step in
// `prep` that produces an artifact the site then reads.
//
// This runs before `astro dev` and before `astro build`, so the figure is
// regenerated on every run of either and says so in the log, next to the other
// prep steps.
//
// Rendering here rather than inside the page also removes the problem that made
// the page version fragile: the components have to be imported and the store
// seeded in one module graph, and doing that from inside an Astro page put the
// seed and the components in different instances, so the tree rendered without
// its workspace. One Vite server, one graph, one seed.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

import { webUiAlias, webUiReactBridge } from "../src/lib/web-ui-alias.mjs";

const APP = fileURLToPath(new URL("../../crates/dux-web/web/", import.meta.url));
const OUT = fileURLToPath(new URL("../src/figure/figure.html", import.meta.url));

// A fresh clone has this repository's source but not the sibling app's
// dependencies, and without them nothing below can resolve. So install them,
// here, rather than telling a contributor to go and do it by hand.
//
// This is what keeps `npm run dev`, `npm run build` and CI identical: one
// mechanism, run from `prep`, so there is no separate setup step to forget in a
// workflow file or in a contributing guide, and no state where the site builds
// on one machine and not another. `npm ci` is idempotent and fast once the
// install exists, so the common case costs a spawn and nothing else.
//
// Install only, never `npm run build`. The figure renders from SOURCE, so
// building the app here would be minutes of work producing output nothing reads.
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
