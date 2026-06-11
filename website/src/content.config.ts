import { defineCollection, z } from "astro:content";
import { glob } from "astro/loaders";

// Docs are plain Markdown files living in `website/docs/`. Drop a new `.md`
// file in there, give it frontmatter, and it becomes a page at
// `/docs/<filename>` automatically. The sidebar, anchors, and table of
// contents are all generated from the file and its headings.
const docs = defineCollection({
  loader: glob({ pattern: "**/*.{md,mdx}", base: "./docs" }),
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

// Blog posts are plain Markdown files in `website/blog/`. Drop a new `.md` file
// in there, give it frontmatter, and it becomes a post at `/blog/<filename>`
// automatically — listed on the blog index, in the RSS feed, in search, and in
// the homepage teaser. Posts are a flat reverse-chronological feed (no tags or
// categories); ordering comes from `pubDate`.
const blog = defineCollection({
  loader: glob({ pattern: "**/*.{md,mdx}", base: "./blog" }),
  schema: z.object({
    // Post title. Shown as the <h1>, the browser tab, and the list label.
    title: z.string(),
    // One-sentence summary. Used for the meta description, the blog index, the
    // RSS item description, and the homepage teaser.
    description: z.string(),
    // Post author. Defaults to the project maintainer. Known authors (see
    // src/lib/authors.ts) get their byline auto-linked to their site; add a
    // contributor there once rather than repeating their URL on each post.
    author: z.string().default("Patrick D'appollonio"),
    // Publish date. Drives reverse-chronological sort order and the RSS
    // pubDate. `coerce` lets the frontmatter write a plain `2026-06-08` string.
    pubDate: z.coerce.date(),
    // Optional last-updated date. When present, shown as "Updated …" under the
    // title.
    updatedDate: z.coerce.date().optional(),
    // Drafts are excluded from the index, RSS, sitemap, and search. They still
    // build, so you can preview one by visiting its URL directly during dev.
    draft: z.boolean().default(false),
  }),
});

export const collections = { docs, blog };
