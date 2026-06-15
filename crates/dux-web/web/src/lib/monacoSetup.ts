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
// JSON has no "basic-language" grammar — its highlighting ships with the real
// JSON language service. We keep that one service (its worker is ~400KB vs the
// 6.6MB ts.worker we dropped) so JSON — the config-file format you edit most —
// gets a proper, well-tested tokenizer plus validation. No other language
// services are registered, so no other worker ships.
import "monaco-editor/esm/vs/language/json/monaco.contribution"
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker"
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker"

import { extensionForPath, fileNameForPath } from "@/lib/pathExt"

// Self-host: point the wrapper at the bundled `monaco` instance and supply the
// workers via Vite `?worker` imports (hashed chunks rust-embed bakes into the
// binary).
self.MonacoEnvironment = {
  getWorker: (_id, label) =>
    label === "json" ? new jsonWorker() : new editorWorker(),
}
loader.config({ monaco })

export { monaco }

// The Monaco language id for a file path, derived from the grammars actually
// registered above (so it stays in sync with `@/monacoLanguages`). Monaco's
// `Editor` infers the language from its `path` prop automatically; `DiffEditor`
// could do the same via `originalModelPath`/`modifiedModelPath`, but the diff
// viewer resolves the language explicitly to avoid creating path-keyed models
// that collide with the editor's. Returns `undefined` (→ plaintext) when no
// registered language claims the extension.
export function monacoLanguageForPath(path: string): string | undefined {
  const ext = extensionForPath(path)
  const file = fileNameForPath(path)
  for (const lang of monaco.languages.getLanguages()) {
    if (ext && lang.extensions?.some((e) => e.toLowerCase() === ext)) {
      return lang.id
    }
    if (lang.filenames?.some((f) => f.toLowerCase() === file)) {
      return lang.id
    }
  }
  return undefined
}
