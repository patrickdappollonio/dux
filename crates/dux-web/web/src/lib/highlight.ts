// highlight.js/lib/common is ~300KB and is only needed when a diff is actually
// viewed. We load it lazily via a module-level singleton promise so the cost
// rides in its own async chunk rather than the initial bundle. Until it
// resolves, diff lines render as escaped plain text; once it loads, subscribers
// re-render with real highlighting.

type Hljs = (typeof import("highlight.js"))["default"]

let hljs: Hljs | null = null
let loadPromise: Promise<void> | null = null
const listeners = new Set<() => void>()

// Kick off (or reuse) the dynamic import. Resolving stores the module and
// notifies every subscriber so they re-render highlighted.
function loadHighlighter(): Promise<void> {
  if (loadPromise) return loadPromise
  loadPromise = import("highlight.js/lib/common").then((m) => {
    hljs = m.default
    for (const listener of listeners) listener()
  })
  return loadPromise
}

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
  return EXT_TO_LANG[ext] ?? null
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
}

// Returns SAFE HTML for one line of code: highlight.js escapes the source text
// itself, so the output never contains unescaped user content. When the
// highlighter has not loaded yet (`ready` false), no language is resolved, or
// the language is not registered in the common bundle, returns the plainly-
// escaped content (still safe HTML). `ready` is the useSyncExternalStore
// snapshot from `getHighlighterReady`; passing it explicitly lets callers list
// it as a genuine memo dependency so highlighting recomputes once the lazy
// module arrives.
export function highlightLine(
  content: string,
  language: string | null,
  ready: boolean,
): string {
  if (content.length === 0) return ""
  if (!ready || !language || !hljs || !hljs.getLanguage(language)) {
    return escapeHtml(content)
  }
  try {
    return hljs.highlight(content, { language, ignoreIllegals: true }).value
  } catch {
    return escapeHtml(content)
  }
}

// useSyncExternalStore plumbing so a component can re-render once the
// highlighter finishes loading. `subscribeHighlighter` also triggers the lazy
// load on first subscription. `getHighlighterReady` reports whether the module
// is available yet — the store snapshot React compares between renders.
export function subscribeHighlighter(onChange: () => void): () => void {
  listeners.add(onChange)
  void loadHighlighter()
  return () => {
    listeners.delete(onChange)
  }
}

export function getHighlighterReady(): boolean {
  return hljs !== null
}
