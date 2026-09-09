#!/usr/bin/env node
// Post-build guard for the web-UI figure, run from `npm run build` rather than
// from the test suite, because the deploy workflow does not run `npm test`.
//
// `astro build` already fails when a component throws, but not on the two silent
// failures: a figure that renders EMPTY because the seeded store stopped reaching
// the components, and a figure given a `client:*` directive, which ships a
// runtime the figure's whole claim denies. So this asserts the built artifacts on
// disk: the page exists, the fabricated workspace is visibly in it, it carries
// the app's stylesheet, it ships no script at all, and the homepage embeds it.

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
// component. If the store seed stops reaching the tree these vanish while the
// wrapper markup stays, so naming them individually says which component
// went quiet.
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
// hydrated island, so any script tag here means a `client:*` directive.
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
