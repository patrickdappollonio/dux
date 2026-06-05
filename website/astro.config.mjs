import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import mdx from "@astrojs/mdx";
import pagefind from "astro-pagefind";
import { unified } from "@astrojs/markdown-remark";
import rehypeSlug from "rehype-slug";
import rehypeAutolinkHeadings from "rehype-autolink-headings";

export default defineConfig({
  site: "https://getdux.app",
  output: "static",
  trailingSlash: "ignore",
  // mdx() inherits the markdown config below (heading anchors, Shiki) so .mdx
  // docs get the same treatment as .md, plus inline components.
  //
  // pagefind() builds the static search index on `astro build` (shipped in
  // dist/pagefind/) and serves a prebuilt index during `astro dev`. Only pages
  // carrying `data-pagefind-body` are indexed — see DocsLayout.astro — so the
  // index stays scoped to the docs and excludes the marketing homepage.
  integrations: [mdx(), pagefind(), sitemap()],
  build: {
    inlineStylesheets: "auto",
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
      ],
    }),
  },
});
