import rss from "@astrojs/rss";
import type { APIContext } from "astro";
import { getPublishedPosts } from "../lib/blog";

// RSS feed for the blog, served at /rss.xml. Sources the same published (non-
// draft), newest-first posts as the blog index. @astrojs/rss handles XML
// escaping, CDATA, and RFC-822 pubDate formatting.
export async function GET(context: APIContext) {
  const posts = await getPublishedPosts();
  const site = context.site ?? new URL("https://getdux.app");

  return rss({
    title: "dux blog",
    description:
      "Updates, release notes, and what's being worked on in dux, the terminal UI for running AI coding agents in parallel.",
    site,
    items: posts.map((post) => ({
      title: post.data.title,
      description: post.data.description,
      pubDate: post.data.pubDate,
      link: `/blog/${post.id}`,
    })),
    customData: "<language>en-us</language>",
  });
}
