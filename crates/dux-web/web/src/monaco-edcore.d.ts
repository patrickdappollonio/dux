// `edcore.main` bundles the Monaco editor and its contributions but omits the language services
// the default `monaco-editor` barrel pulls in. It ships no .d.ts, so reuse the full package's
// types: the runtime namespace it exports is the same editor/languages API.
declare module "monaco-editor/esm/vs/editor/edcore.main" {
  export * from "monaco-editor"
}

// The per-language grammar contributions are side-effect-only and do not resolve their bundled
// .d.ts under bundler module resolution, so this wildcard makes them importable.
declare module "monaco-editor/esm/vs/basic-languages/*"
