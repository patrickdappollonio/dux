import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import rehypeSlug from "rehype-slug";
import rehypeAutolinkHeadings from "rehype-autolink-headings";

export default defineConfig({
  site: "https://getdux.app",
  output: "static",
  trailingSlash: "ignore",
  integrations: [sitemap()],
  build: {
    inlineStylesheets: "auto",
  },
  markdown: {
    // GitHub's dark theme reads cleanly on the site's near-black panels and
    // ships its token colors calibrated for that background.
    shikiConfig: { theme: "github-dark-default", wrap: false },
    rehypePlugins: [
      // Give every heading a stable slug id, then append a clickable "#"
      // anchor so docs headings are linkable.
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
  },
});
