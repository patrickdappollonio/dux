// Shared Monaco self-host bootstrap, imported by every component that mounts a
// Monaco surface. Importing this module runs the setup exactly once (ES modules
// are singletons), so `Editor` and `DiffEditor` both render against the bundled
// `monaco` instance with workers wired, and no CDN is used: dux serves the SPA
// offline.
//
// `monaco-editor/editor` is the bare editor API. The two `register.all`
// imports opt into every editor feature (find, suggest, hover, folding...) and
// every Monarch language definition, and deliberately NOT into
// `languages/features`, the typescript/json/css/html language services the
// default `monaco-editor` entry registers. So only the editor worker ships
// (`ts.worker` alone is several MB) and highlighting runs on the main thread
// from the Monarch grammars. What that gives up is IntelliSense and
// diagnostics. Each definition loads its grammar on first use, and consumers
// are lazy-loaded, so Monaco stays out of the main bundle until the editor
// opens.
import { loader } from "@monaco-editor/react"
import * as monaco from "monaco-editor/editor"
import "monaco-editor/features/register.all"
import "monaco-editor/languages/definitions/register.all"
import editorWorker from "monaco-editor/editor/editor.worker?worker"

import { inferredLanguageId } from "@/lib/editorLanguage"
import {
  DIFF_BODY_RULES,
  DIFF_HEADER_RULES,
  DIFF_TOKEN,
  type DiffRule,
} from "@/lib/diffGrammar"
import { holdNewModelsInLf } from "@/lib/monacoModelEol"
import { tokenHex } from "@/lib/tokenHex"

// Self-host: point the wrapper at the bundled `monaco` instance and supply the
// one worker via a Vite `?worker` import (a hashed chunk rust-embed bakes into
// the binary). No language service is registered, so no language-service worker
// exists to route to.
self.MonacoEnvironment = {
  getWorker: () => new editorWorker(),
}
loader.config({ monaco })
// Every model is LF from the moment it is made, on every browser platform
// (see `holdNewModelsInLf`).
holdNewModelsInLf(monaco)

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
// reason anyone reads a diff. The rules live in `diffGrammar` as data so the
// tokens they emit can be tested without standing Monaco up.
function monarchRules(rules: readonly DiffRule[]): [RegExp, unknown][] {
  return rules.map((rule) => [
    rule.pattern,
    rule.next ? { token: rule.token, next: `@${rule.next}` } : rule.token,
  ])
}

if (!monaco.languages.getLanguages().some((l) => l.id === "diff")) {
  monaco.languages.register({ id: "diff", extensions: [".diff", ".patch"], aliases: ["Diff"] })
  const diff = {
    tokenizer: {
      root: monarchRules(DIFF_HEADER_RULES),
      header: monarchRules(DIFF_HEADER_RULES),
      body: monarchRules(DIFF_BODY_RULES),
    },
  } as unknown as monaco.languages.IMonarchLanguage
  monaco.languages.setMonarchTokensProvider("diff", diff)
}

// The theme the patch viewer renders under. vs-dark has no diff scopes, so its
// defaults land on `string` (salmon) and `comment` (green): additions read as
// removals and removals as additions, which is worse than no colour at all.
// The two content colours are the `--dux-added` and `--dux-deleted` tokens the
// changed-files pane counts additions and deletions in, resolved to hex because
// Monaco's rules take nothing else. The theme is defined just before the editor
// mounts, when the stylesheet is certain to have applied, and each mount reads
// the tokens again. The fallbacks are those tokens' Tailwind green-500 and
// red-500. Only the props pair below is exported: an editor handed the name
// without the definer would render under Monaco's fallback theme.
const DIFF_THEME = "dux-diff"
function defineDiffTheme(): void {
  monaco.editor.defineTheme(DIFF_THEME, {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: DIFF_TOKEN.inserted, foreground: tokenHex("--dux-added", "00C950") },
      { token: DIFF_TOKEN.deleted, foreground: tokenHex("--dux-deleted", "FB2C36") },
      { token: DIFF_TOKEN.header, foreground: "9CA3AF", fontStyle: "bold" },
      { token: DIFF_TOKEN.hunk, foreground: "38BDF8" },
    ],
    colors: {},
  })
}
export const DIFF_THEME_PROPS = {
  theme: DIFF_THEME,
  beforeMount: defineDiffTheme,
} as const

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
