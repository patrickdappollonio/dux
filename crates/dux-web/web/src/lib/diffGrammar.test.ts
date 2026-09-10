import { describe, expect, it } from "vitest"
import { DIFF_TOKEN, diffLineToken, diffTokens } from "./diffGrammar"

describe("diffTokens", () => {
  it("names the token for every kind of line in a patch", () => {
    const patch = [
      "diff --git a/a.txt b/a.txt",
      "index 1234567..89abcde 100644",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,3 +1,3 @@",
      " context",
      "-gone",
      "+arrived",
    ].join("\n")
    expect(diffTokens(patch)).toEqual([
      DIFF_TOKEN.header,
      DIFF_TOKEN.header,
      DIFF_TOKEN.header,
      DIFF_TOKEN.header,
      DIFF_TOKEN.hunk,
      DIFF_TOKEN.text,
      DIFF_TOKEN.deleted,
      DIFF_TOKEN.inserted,
    ])
  })

  it("reads a removed SQL comment as removed content, not a file header", () => {
    // A REMOVED line whose content is `-- foo` is printed as `--- foo`: the
    // patch's own `-` marker plus the comment's two dashes, which is exactly
    // what a file header looks like. Same for an added `++ bar`, printed as
    // `+++ bar`. Past the first hunk they are content, and colouring them as
    // chrome loses the one thing a diff is read for.
    const patch = [
      "@@ -1,2 +1,2 @@",
      "--- the old comment",
      "+++ the new one",
    ].join("\n")
    expect(diffTokens(patch)).toEqual([
      DIFF_TOKEN.hunk,
      DIFF_TOKEN.deleted,
      DIFF_TOKEN.inserted,
    ])
  })

  it("returns to header rules at the next file in a multi-file patch", () => {
    const patch = [
      "@@ -1 +1 @@",
      "-one",
      "diff --git a/b.txt b/b.txt",
      "--- a/b.txt",
      "+++ b/b.txt",
      "@@ -1 +1 @@",
      "+two",
    ].join("\n")
    expect(diffTokens(patch)).toEqual([
      DIFF_TOKEN.hunk,
      DIFF_TOKEN.deleted,
      DIFF_TOKEN.header,
      DIFF_TOKEN.header,
      DIFF_TOKEN.header,
      DIFF_TOKEN.hunk,
      DIFF_TOKEN.inserted,
    ])
  })

  it("keeps the no-newline marker out of the content colours", () => {
    expect(diffLineToken("\\ No newline at end of file", "body").token).toBe(
      DIFF_TOKEN.hunk,
    )
  })

  it("never colours an addition with a token vs-dark paints salmon", () => {
    // The bug this grammar replaced: `+` lines carried Monaco's `string` scope
    // and `-` lines its `comment` scope, so additions rendered salmon and
    // deletions green. The token names must not be either of those.
    const tokens = new Set(diffTokens("@@ -1 +1 @@\n+added\n-removed"))
    expect(tokens.has("string")).toBe(false)
    expect(tokens.has("comment")).toBe(false)
    expect(tokens.has(DIFF_TOKEN.inserted)).toBe(true)
    expect(tokens.has(DIFF_TOKEN.deleted)).toBe(true)
  })
})
