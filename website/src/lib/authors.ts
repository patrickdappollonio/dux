// Known blog authors and their personal links. Add a contributor here once and
// every post with `author: "<their name>"` gets their byline auto-linked — posts
// never repeat the URL. An author not listed here renders as plain text (no
// link), so a one-off guest writer still works without an entry.

export interface Author {
  name: string;
  /** Personal site. Absent for authors with no link. */
  url?: string;
}

const AUTHOR_LINKS: Record<string, string> = {
  "Patrick D'appollonio": "https://www.patrickdap.com",
};

// Resolve an author name to a name + optional link.
export function getAuthor(name: string): Author {
  const url = AUTHOR_LINKS[name];
  return url ? { name, url } : { name };
}
