import hljs from "highlight.js/lib/common"

// Map a file path's extension to a highlight.js language name. Returns null for
// unknown/unsupported extensions (caller renders escaped plain text instead).
const EXT_TO_LANG: Record<string, string> = {
  ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  rs: "rust", go: "go", py: "python", rb: "ruby", java: "java",
  c: "c", h: "c", cpp: "cpp", cc: "cpp", hpp: "cpp", cs: "csharp",
  php: "php", swift: "swift", kt: "kotlin", scala: "scala", lua: "lua",
  sh: "bash", bash: "bash", zsh: "bash", fish: "bash",
  json: "json", yaml: "yaml", yml: "yaml", toml: "ini", ini: "ini",
  md: "markdown", markdown: "markdown",
  css: "css", scss: "scss", less: "less", html: "xml", xml: "xml", svg: "xml",
  sql: "sql", graphql: "graphql", gql: "graphql", diff: "diff", patch: "diff",
}

export function languageForPath(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  const lang = EXT_TO_LANG[ext]
  if (!lang) return null
  // Guard against a language not registered in the common bundle.
  return hljs.getLanguage(lang) ? lang : null
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
}

// Returns SAFE HTML for one line of code: highlight.js escapes the source text
// itself, so the output never contains unescaped user content. When no language
// is resolved, returns the plainly-escaped content (still safe HTML).
export function highlightLine(content: string, language: string | null): string {
  if (content.length === 0) return ""
  if (!language) return escapeHtml(content)
  try {
    return hljs.highlight(content, { language, ignoreIllegals: true }).value
  } catch {
    return escapeHtml(content)
  }
}
