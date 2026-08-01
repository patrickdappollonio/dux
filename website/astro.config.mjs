import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import mdx from "@astrojs/mdx";
import react from "@astrojs/react";
import { webUiAlias } from "./src/lib/web-ui-alias.mjs";
import pagefind from "astro-pagefind";
import tailwindcss from "@tailwindcss/vite";
import { unified } from "@astrojs/markdown-remark";
import rehypeSlug from "rehype-slug";
import rehypeAutolinkHeadings from "rehype-autolink-headings";
import rehypeProseImages from "./src/lib/rehype-prose-images.mjs";
import remarkGemoji from "remark-gemoji";
import remarkAdmonitions from "./src/lib/remark-admonitions.mjs";

export default defineConfig({
  site: "https://getdux.app",
  output: "static",
  trailingSlash: "ignore",
  // mdx() inherits the markdown config below (heading anchors, Shiki) so .mdx
  // docs get the same treatment as .md, plus inline components.
  //
  // pagefind() builds the static search index on `astro build` (shipped in
  // dist/pagefind/) and serves a prebuilt index during `astro dev`. Only pages
  // carrying `data-pagefind-body` are indexed — the docs (DocsLayout.astro) and
  // blog posts (BlogLayout.astro) — so the index covers docs and blog while
  // excluding the marketing homepage.
  // The sitemap excludes the RSS endpoint (it's a feed, not a page). Draft
  // posts never reach the sitemap because they're dropped from the production
  // build entirely (see src/pages/blog/[...slug].astro).
  integrations: [
    mdx(),
    // React is here for ONE thing: the homepage's web-UI figure, which renders
    // the dux web app's real components (from crates/dux-web/web/src) to static
    // HTML at build time. No page carries a `client:*` directive, so no React
    // runtime is shipped to any visitor. Keep it that way — the figure's whole
    // claim is that it is the real UI with zero JavaScript behind it.
    react(),
    pagefind(),
    // Newsletter status pages are post-subscribe/post-confirm landing pages
    // (noindex), so they stay out of the sitemap too. /figure/ is the web-UI
    // figure's embed target, not a destination, so it stays out too.
    sitemap({
      filter: (page) =>
        !page.endsWith("/rss.xml") &&
        !page.includes("/newsletter/") &&
        !page.includes("/figure/"),
    }),
  ],
  build: {
    inlineStylesheets: "auto",
  },
  // Tailwind 4 through its own Vite plugin, which is the path Astro and
  // Tailwind both document. The PostCSS plugin is not usable here: under
  // Vite 8 the bundled postcss-import resolves `@import "tailwindcss"` in
  // global.css as a relative file and fails before Tailwind's plugin ever
  // runs. The Vite plugin resolves the bare package import itself.
  vite: {
    plugins: [tailwindcss()],
    // The web-UI figure imports the dux web app's components straight out of
    // `crates/dux-web/web/src`. They resolve through the app's own `@` alias,
    // and every React copy in play has to collapse to one. Shared with the test
    // runner (`vitest.config.ts`) so the drift guard renders the figure exactly
    // the way this build does; see `src/lib/web-ui-alias.mjs`.
    resolve: { alias: webUiAlias() },
    build: {
      // Match Tailwind's own compile targets. Tailwind emits the vendor
      // prefixes those browsers need (notably `-webkit-backdrop-filter`, which
      // Safari required before 18 and the sticky header's blur depends on),
      // but Vite 8 minifies the result with Lightning CSS against a newer
      // default target and strips them back out. Stating the floor here keeps
      // the prefixes in the shipped CSS.
      cssTarget: ["safari16.4", "chrome111", "firefox128", "edge111"],
    },
  },
  markdown: {
    // GitHub's dark theme reads cleanly on the site's near-black panels and
    // ships its token colors calibrated for that background. shikiConfig and
    // syntaxHighlight stay at the markdown level — Astro forwards them to the
    // processor's renderer, so highlighting is unaffected by the processor.
    shikiConfig: { theme: "github-dark-default", wrap: false },
    // Astro 6 deprecated top-level markdown.rehypePlugins/remarkPlugins in
    // favor of a processor built with unified() from @astrojs/markdown-remark.
    processor: unified({
      // GitHub-style emoji shortcodes (`:smile:` -> 😄) in any Markdown page.
      // Operates on text nodes only, so shortcodes inside code spans/blocks are
      // left literal.
      //
      // remarkAdmonitions turns GitHub-style `> [!NOTE]` blockquotes into styled
      // alert callouts (see src/lib/remark-admonitions.mjs; styled in global.css).
      remarkPlugins: [remarkGemoji, remarkAdmonitions],
      rehypePlugins: [
        // Give every heading a stable slug id, then append a clickable "#"
        // anchor so docs headings are linkable. The slug ids also power the
        // heading-level deep links in docs search (see DocsSearch.astro).
        rehypeSlug,
        [
          rehypeAutolinkHeadings,
          {
            behavior: "append",
            properties: {
              className: ["heading-anchor"],
              ariaHidden: "true",
              tabIndex: -1,
            },
            // Empty anchor: the visible "#" is added via CSS so it never leaks
            // into the heading text that the table of contents is built from.
            content: { type: "element", tagName: "span", properties: {}, children: [] },
          },
        ],
        // Markdown image upgrades: `#left|#right|#center|#full` alignment via
        // the URL hash, plus a <picture>/webp wrapper for local raster images.
        rehypeProseImages,
      ],
    }),
  },
});
