import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import mdx from "@astrojs/mdx";
import react from "@astrojs/react";
import { webUiAlias, webUiReactBridge, WEB_UI_ROOT } from "./src/lib/web-ui-alias.mjs";
import pagefind from "astro-pagefind";
import tailwindcss from "@tailwindcss/vite";
import { unified } from "@astrojs/markdown-remark";
import rehypeSlug from "rehype-slug";
import rehypeAutolinkHeadings from "rehype-autolink-headings";
import rehypeProseImages from "./src/lib/rehype-prose-images.mjs";
import remarkGemoji from "remark-gemoji";
import remarkAdmonitions from "./src/lib/remark-admonitions.mjs";
import remarkGraphviz from "./src/lib/remark-graphviz.mjs";

export default defineConfig({
  site: "https://getdux.app",
  output: "static",
  trailingSlash: "ignore",
  // mdx() inherits the markdown config below, so .mdx docs get the same treatment
  // as .md. pagefind() indexes only pages carrying `data-pagefind-body`, which is
  // the docs and blog layouts, so the marketing homepage stays out. The sitemap
  // excludes the RSS endpoint; draft posts never reach it because the production
  // build drops them entirely.
  integrations: [
    mdx(),
    // React is here only for the homepage's web-UI figure. No page carries a
    // `client:*` directive, so no React runtime reaches a visitor, and the
    // figure's whole claim is that it is the real UI with zero JavaScript.
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
  // Tailwind 4 through its Vite plugin, never the PostCSS one: under Vite 8 the
  // bundled postcss-import resolves `@import "tailwindcss"` as a relative file
  // and fails before Tailwind's plugin runs.
  vite: {
    plugins: [webUiReactBridge(), tailwindcss()],
    server: {
      fs: {
        // Vite resolves a module outside its root but refuses to SERVE a file
        // from there, and the figure's components load real assets out of the
        // app's dependencies. Only the app's directory is added: this is a
        // dev-server read permission, so it stays as narrow as its purpose.
        allow: [".", WEB_UI_ROOT],
      },
    },
    // The figure's components resolve through the app's own `@` alias, and every
    // React copy in play has to collapse to one. Shared with `vitest.config.ts`
    // so the drift guard renders the figure exactly the way this build does.
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
    // GitHub's dark theme ships its token colors calibrated for the near-black
    // background the site's panels use. shikiConfig and syntaxHighlight stay at
    // the markdown level, from where Astro forwards them to the renderer.
    shikiConfig: { theme: "github-dark-default", wrap: false },
    // Astro 6 deprecated top-level markdown.rehypePlugins/remarkPlugins in
    // favor of a processor built with unified() from @astrojs/markdown-remark.
    processor: unified({
      // Emoji shortcodes operate on text nodes only, so shortcodes inside code
      // spans and blocks stay literal. remarkGraphviz runs last because it
      // replaces its nodes with raw HTML.
      remarkPlugins: [remarkGemoji, remarkAdmonitions, remarkGraphviz],
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
