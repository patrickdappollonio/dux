import { defineCollection, z } from "astro:content";
import { glob } from "astro/loaders";

// Docs are plain Markdown files living in `website/docs/`. Drop a new `.md`
// file in there, give it frontmatter, and it becomes a page at
// `/docs/<filename>` automatically. The sidebar, anchors, and table of
// contents are all generated from the file and its headings.
const docs = defineCollection({
  loader: glob({ pattern: "**/*.md", base: "./docs" }),
  schema: z.object({
    // Page title. Shown as the <h1>, the browser tab, and the sidebar label.
    title: z.string(),
    // One-sentence summary. Used for the meta description and the docs index.
    description: z.string(),
    // Sidebar section this page belongs under. Pages with the same group are
    // listed together.
    group: z.string().default("Guides"),
    // Sort order within a group and across groups. Lower floats to the top.
    order: z.number().default(100),
  }),
});

export const collections = { docs };
