// Shared Monaco self-host bootstrap, imported by every component that mounts a
// Monaco surface. Importing this module runs the setup exactly once (ES modules
// are singletons), so `Editor` and `DiffEditor` both render against the bundled
// `monaco` instance with workers wired, and no CDN is used: dux serves the SPA
// offline.
//
// `edcore.main` is the editor core and its contributions without the
// typescript/json/css/html language services the default `monaco-editor` barrel
// registers, so only the editor worker ships (`ts.worker` alone is ~6.6MB) and
// highlighting runs on the main thread from the Monarch grammars below. What
// that gives up is IntelliSense and diagnostics. Consumers are lazy-loaded, so
// even the trimmed Monaco stays out of the main bundle until the editor opens.
import { loader } from "@monaco-editor/react"
import * as monaco from "monaco-editor/esm/vs/editor/edcore.main"
import "@/monacoLanguages"
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker"

import { inferredLanguageId } from "@/lib/editorLanguage"

// Self-host: point the wrapper at the bundled `monaco` instance and supply the
// one worker via a Vite `?worker` import (a hashed chunk rust-embed bakes into
// the binary). No language service is registered, so no language-service worker
// exists to route to.
self.MonacoEnvironment = {
  getWorker: () => new editorWorker(),
}
loader.config({ monaco })

// Monaco ships no TOML grammar, so `config.toml` would fall back to plaintext.
// A minimal Monarch tokenizer instead: highlighting only, no language service.
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

// JSON's stock highlighting ships only with the JSON language service, whose
// ~400KB worker buys schema validation dux does not use, so JSON gets the same
// treatment as TOML: a Monarch tokenizer, no language service, no worker.
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

// Monaco ships no diff grammar either, and the head of git's own patch is the
// one place the web renders raw diff text (see DiffHeadViewer). Without this it
// would be plaintext, losing the added/removed colours, which are the whole
// reason anyone reads a diff.
if (!monaco.languages.getLanguages().some((l) => l.id === "diff")) {
  monaco.languages.register({ id: "diff", extensions: [".diff", ".patch"], aliases: ["Diff"] })
  const diff: monaco.languages.IMonarchLanguage = {
    tokenizer: {
      root: [
        // File headers first: they lead with the same characters as the
        // added/removed lines below and would otherwise be coloured as content.
        [/^diff .*$/, "keyword"],
        [/^(?:index|new file mode|deleted file mode|similarity index|rename from|rename to|old mode|new mode) .*$/, "keyword"],
        [/^(?:---|\+\+\+) .*$/, "keyword"],
        [/^@@.*$/, "type"],
        [/^\+.*$/, "string"],
        [/^-.*$/, "comment"],
        [/^\\.*$/, "type"],
      ],
    },
  }
  monaco.languages.setMonarchTokensProvider("diff", diff)
}

export { monaco }

// The Monaco language id for a file path, from the grammars registered above.
// `Editor` infers the language from its `path` prop, but the diff viewer
// resolves it explicitly to avoid creating path-keyed models that collide with
// the editor's. `undefined` when nothing claims the extension, which is
// plaintext.
export function monacoLanguageForPath(path: string): string | undefined {
  // The walk lives in `lib/editorLanguage`, which takes the registry as an
  // argument so it can be unit-tested; the language picker resolves the same
  // question through it, which is what keeps the two from drifting apart.
  return inferredLanguageId(path, monaco.languages.getLanguages())
}
