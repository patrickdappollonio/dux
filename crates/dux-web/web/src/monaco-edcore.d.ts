// `edcore.main` bundles the Monaco editor plus all editor contributions (find,
// folding, bracket matching, …) but omits the language services (typescript/
// json/css/html) the default `monaco-editor` barrel pulls in. It ships no .d.ts,
// so reuse the full package types — the runtime namespace it exports is the same
// editor/languages API.
declare module "monaco-editor/esm/vs/editor/edcore.main" {
  export * from "monaco-editor"
}

// The per-language grammar contributions (registered for syntax highlighting in
// monacoLanguages.ts) are side-effect-only and don't resolve their bundled .d.ts
// under bundler module resolution. This wildcard makes them importable.
declare module "monaco-editor/esm/vs/basic-languages/*"

// Same for the JSON language-service contribution (the one language service we
// keep, for real JSON tokenization + validation).
declare module "monaco-editor/esm/vs/language/json/monaco.contribution"
