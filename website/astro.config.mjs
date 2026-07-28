import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import mdx from "@astrojs/mdx";
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
    pagefind(),
    // Newsletter status pages are post-subscribe/post-confirm landing pages
    // (noindex), so they stay out of the sitemap too.
    sitemap({
      filter: (page) =>
        !page.endsWith("/rss.xml") && !page.includes("/newsletter/"),
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
