#!/usr/bin/env node
// Post-build guard for the web-UI figure. Run after `astro build`, as part of
// `npm run build`, so it gates every build there is: local, PR CI, and the
// deploy workflow (which does NOT run `npm test`, so a check that lived only in
// the test suite would not stand between a broken figure and production).
//
// The figure renders the dux web app's REAL components to static HTML. Three
// things can go wrong, and only the first of them is loud on its own:
//
//   1. A component throws while rendering. `astro build` already fails on that.
//   2. It renders, but EMPTY, because the seeded store stopped reaching the
//      components. Astro is perfectly happy to emit an empty box.
//   3. Someone gives the figure a `client:*` directive, or a component grows a
//      dependency that drags a runtime in. The figure's whole claim is that it
//      is the real UI with zero JavaScript behind it; a hydrated figure is a
//      different (and much heavier) thing wearing the same caption.
//
// So this asserts the built artifacts on disk: the figure page exists, the
// fabricated workspace is visibly in it, it carries the app's stylesheet, it
// ships no script at all, and the homepage still embeds it.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const distDir = resolve(here, "..", "dist");
const figurePage = join(distDir, "figure", "web-ui", "index.html");
const homePage = join(distDir, "index.html");

function fail(message) {
  console.error(`verify-figure: ${message}`);
  process.exit(1);
}

if (!existsSync(figurePage)) {
  fail(
    "dist/figure/web-ui/index.html is missing. The homepage embeds it, so the " +
      "web-UI section would render an empty frame.",
  );
}

const figure = readFileSync(figurePage, "utf8");

// Content from the fabricated workspace, each piece owned by a different real
// component: the sidebar's agent rows and its terminals section, the header
// crumbs, the PR lane, the tab strip, and the changed-files pane. If the store
// seed stops reaching the tree these vanish while the wrapper markup stays, so
// naming them individually says WHICH component went quiet.
const EXPECTED = [
  ["the focused agent (sidebar + header)", "checkout-retry"],
  ["a sibling agent (sidebar flat list)", "webhook-replay"],
  ["a project name (sidebar)", "storefront"],
  ["a second project (sidebar)", "billing-api"],
  ["the focused branch (header crumbs)", "dux/checkout-retry"],
  ["a project terminal (sidebar terminals section)", "npm run dev"],
  ["the pull request (PR lane)", "482"],
  ["a staged file (changes pane)", "retry-policy.ts"],
  ["an unstaged file (changes pane)", "CheckoutSummary.tsx"],
];

for (const [what, needle] of EXPECTED) {
  if (!figure.includes(needle)) {
    fail(
      `the figure page is missing ${what}: expected to find ${JSON.stringify(needle)}. ` +
        "The components rendered but the seeded workspace did not reach them.",
    );
  }
}

// The app's own stylesheet. Without it the real markup renders as unstyled
// nested divs, which looks broken rather than absent and would be easy to miss.
if (!/<link[^>]+rel="stylesheet"/.test(figure)) {
  fail(
    "the figure page links no stylesheet. It renders the app's components, so " +
      "without the app's CSS it is a pile of unstyled divs.",
  );
}

// Zero client JavaScript, the hard constraint. Astro only emits a <script> for a
// hydrated island, so any script tag here means someone added a `client:*`
// directive to the figure.
if (/<script/i.test(figure)) {
  fail(
    "the figure page ships a <script>. The figure must render at build time " +
      "with no hydration; check for a `client:*` directive on <WebUIFigure />.",
  );
}

// And the homepage has to actually embed it.
if (!existsSync(homePage)) fail("dist/index.html is missing.");
const home = readFileSync(homePage, "utf8");
if (!home.includes('src="/figure/web-ui"')) {
  fail(
    "the homepage does not embed /figure/web-ui. The figure was built but " +
      "nothing on the site shows it.",
  );
}

console.log("verify-figure: the web-UI figure is present, populated and script-free.");
