// The highlighting rules for a unified patch, as data both the Monaco setup and
// its tests read. Monaco ships no diff grammar, and the head of git's own patch
// is the one place the web renders raw diff text (see DiffHeadViewer).
//
// Header-ness is decided by POSITION, not by the leading characters: after the
// first `@@` a line beginning `-- ` is a removed SQL comment, and a line
// beginning `++ ` is added content. Only before the first hunk of a file do
// `---` and `+++` name the two sides.

// The token names the theme colours. Deliberately not Monaco's stock scopes
// (`string`, `comment`), whose vs-dark colours are salmon and green, which is
// the wrong way round for a diff.
export const DIFF_TOKEN = {
  header: "header",
  hunk: "hunk",
  inserted: "inserted",
  deleted: "deleted",
  text: "",
} as const

export type DiffState = "header" | "body"

export interface DiffRule {
  pattern: RegExp
  token: string
  // The state the tokenizer moves to after this line, when it moves at all.
  next?: DiffState
}

// Before the first hunk of a file: everything git prints about the file itself.
export const DIFF_HEADER_RULES: readonly DiffRule[] = [
  { pattern: /^@@.*$/, token: DIFF_TOKEN.hunk, next: "body" },
  {
    pattern:
      /^(?:diff |index |new file mode |deleted file mode |similarity index |dissimilarity index |rename from |rename to |copy from |copy to |old mode |new mode |Binary files |GIT binary patch|--- |\+\+\+ ).*$/,
    token: DIFF_TOKEN.header,
  },
  { pattern: /^.*$/, token: DIFF_TOKEN.text },
]

// Inside a hunk. A fresh `diff --git` starts the next file, so the header rules
// take over again; a multi-file patch is the ordinary case here.
export const DIFF_BODY_RULES: readonly DiffRule[] = [
  { pattern: /^diff --git .*$/, token: DIFF_TOKEN.header, next: "header" },
  { pattern: /^@@.*$/, token: DIFF_TOKEN.hunk },
  { pattern: /^\+.*$/, token: DIFF_TOKEN.inserted },
  { pattern: /^-.*$/, token: DIFF_TOKEN.deleted },
  // git's "\ No newline at end of file" marker: about the patch, not content.
  { pattern: /^\\.*$/, token: DIFF_TOKEN.hunk },
  { pattern: /^.*$/, token: DIFF_TOKEN.text },
]

// Tokenize one line the way the Monaco tokenizer will, from the same rules in
// the same order, and report the state the next line starts in.
export function diffLineToken(
  line: string,
  state: DiffState,
): { token: string; next: DiffState } {
  const rules = state === "header" ? DIFF_HEADER_RULES : DIFF_BODY_RULES
  for (const rule of rules) {
    if (rule.pattern.test(line)) {
      return { token: rule.token, next: rule.next ?? state }
    }
  }
  return { token: DIFF_TOKEN.text, next: state }
}

// Every token name in one line of a patch, for a test or a caller that wants
// the whole picture rather than a line at a time.
export function diffTokens(text: string): string[] {
  let state: DiffState = "header"
  return text.split("\n").map((line) => {
    const answer = diffLineToken(line, state)
    state = answer.next
    return answer.token
  })
}
