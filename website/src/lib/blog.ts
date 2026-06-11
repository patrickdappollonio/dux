import { getCollection, type CollectionEntry } from "astro:content";

export type BlogPost = CollectionEntry<"blog">;

// Published posts, newest first. Drafts are excluded everywhere a post would be
// publicly listed — the blog index, the RSS feed, the sitemap, and the homepage
// teaser all source from here so they can't drift on which posts are visible.
// (Drafts still build so their URL can be previewed during dev; see the blog
// route's getStaticPaths.)
export async function getPublishedPosts(): Promise<BlogPost[]> {
  const posts = await getCollection("blog", ({ data }) => !data.draft);
  return posts.sort(
    (a, b) => b.data.pubDate.getTime() - a.data.pubDate.getTime(),
  );
}
