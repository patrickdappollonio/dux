import type { RegisteredLanguage } from "@/lib/editorLanguage"

// Monaco's language registry, read through a dynamic import so the module that
// asks never pulls the Monaco bundle into the main chunk: it resolves to the
// chunk CodeEditor and DiffViewer already lazy-load.
//
// Its own module, imported statically, so a test replaces the whole read with
// one static mock. A test that mocked `@/lib/monacoSetup` alone was trusting
// vitest to intercept this dynamic import on every run, and now and then it
// did not: real Monaco then loaded under jsdom and threw an unhandled error
// into whichever test was running.
export function loadRegisteredLanguages(): Promise<RegisteredLanguage[]> {
  return import("@/lib/monacoSetup").then((m) => m.monaco.languages.getLanguages())
}
