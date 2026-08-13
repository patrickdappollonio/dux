// Shared Monaco self-host bootstrap, imported by every component that mounts a
// Monaco surface (the code editor AND the diff viewer). Importing this module
// runs the setup exactly once (ES modules are singletons), so `@monaco-editor/
// react`'s `Editor` and `DiffEditor` both render against the bundled `monaco`
// instance with workers wired — no CDN (dux serves the SPA offline).
//
// `edcore.main` is the editor core + all editor contributions (find, folding,
// bracket matching, …) WITHOUT the typescript/json/css/html language services
// the default `monaco-editor` barrel registers. We then add only the Monarch
// GRAMMARS for syntax highlighting. The result ships just the editor worker — not
// the multi-MB language-service workers (`ts.worker` alone is ~6.6MB) — and drops
// the language-service client code. Highlighting runs on the main thread; what we
// give up is IntelliSense/diagnostics, which add nothing for single-file worktree
// viewing. Consumers are lazy-loaded (React.lazy) so even the trimmed Monaco
// never touches the main bundle until the editor opens.
import { loader } from "@monaco-editor/react"
import * as monaco from "monaco-editor/esm/vs/editor/edcore.main"
import "@/monacoLanguages"
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker"

import { inferredLanguageId } from "@/lib/editorLanguage"

// Self-host: point the wrapper at the bundled `monaco` instance and supply the
// one worker via a Vite `?worker` import (a hashed chunk rust-embed bakes into
// the binary). Only the editor worker ships: no language service is registered,
// so no language-service worker exists to route to. (The JSON language service
// was the last one — its ~400KB worker bought schema validation dux never
// used; JSON coloring now comes from the Monarch grammar below.)
self.MonacoEnvironment = {
  getWorker: () => new editorWorker(),
}
loader.config({ monaco })

// Monaco ships no TOML grammar (it is not one of the basic-languages), so a
// path of `config.toml` would otherwise fall back to plaintext. Register a
// minimal Monarch tokenizer for the config editor: comments, table headers,
// keys, strings, numbers, and booleans. Highlighting only — no language service.
if (!monaco.languages.getLanguages().some((l) => l.id === "toml")) {
  monaco.languages.register({ id: "toml", extensions: [".toml"], aliases: ["TOML"] })
  const toml: monaco.languages.IMonarchLanguage = {
    tokenizer: {
      root: [
        [/#.*$/, "comment"],
        [/^\s*\[\[?[^\]]*\]\]?/, "type"],
        [/[A-Za-z0-9_.-]+(?=\s*=)/, "variable"],
        [/=/, "operator"],
        [/"""/, { token: "string", next: "@mlstring" }],
        [/"/, { token: "string", next: "@string" }],
        [/'[^']*'/, "string"],
        [/\b(?:true|false)\b/, "keyword"],
        [/[+-]?\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?/, "number"],
      ],
      string: [
        [/[^"\\]+/, "string"],
        [/\\./, "string.escape"],
        [/"/, { token: "string", next: "@pop" }],
      ],
      mlstring: [
        [/"""/, { token: "string", next: "@pop" }],
        [/./, "string"],
      ],
    },
  }
  monaco.languages.setMonarchTokensProvider("toml", toml)
}

// JSON has no "basic-language" grammar either — its stock highlighting ships
// only with the JSON language service, whose ~400KB worker exists for schema
// validation dux never used. So JSON gets the same treatment as TOML: a minimal
// Monarch tokenizer (property keys, strings, numbers, keywords). Highlighting
// only — no language service, no worker.
if (!monaco.languages.getLanguages().some((l) => l.id === "json")) {
  monaco.languages.register({ id: "json", extensions: [".json"], aliases: ["JSON"] })
  const json: monaco.languages.IMonarchLanguage = {
    tokenizer: {
      root: [
        [/"(?:[^"\\]|\\.)*"(?=\s*:)/, "type"],
        [/"(?:[^"\\]|\\.)*"/, "string"],
        [/\b(?:true|false|null)\b/, "keyword"],
        [/-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/, "number"],
        [/[{}[\],:]/, "delimiter"],
      ],
    },
  }
  monaco.languages.setMonarchTokensProvider("json", json)
}

export { monaco }

// The Monaco language id for a file path, derived from the grammars actually
// registered above (so it stays in sync with `@/monacoLanguages`). Monaco's
// `Editor` infers the language from its `path` prop automatically; `DiffEditor`
// could do the same via `originalModelPath`/`modifiedModelPath`, but the diff
// viewer resolves the language explicitly to avoid creating path-keyed models
// that collide with the editor's. Returns `undefined` (→ plaintext) when no
// registered language claims the extension.
export function monacoLanguageForPath(path: string): string | undefined {
  // The walk itself lives in `lib/editorLanguage`, which takes the registry as
  // an argument and so can be unit-tested; this only supplies the registry.
  // The language PICKER resolves the same question through the same function,
  // which is what keeps the two from drifting apart.
  return inferredLanguageId(path, monaco.languages.getLanguages())
}
