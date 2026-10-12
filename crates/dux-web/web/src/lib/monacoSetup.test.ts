import { describe, expect, it } from "vitest"

// Monaco cannot mount under vitest (see editorBuffers.test.ts), so this pins
// the SOURCE of the bootstrap instead: dux ships no language services at all,
// only Monarch grammars. The JSON language service was the last one standing:
// its ~400KB json.worker bought schema validation nothing in dux uses, so its
// contribution import must never come back. JSON keeps syntax coloring through
// the hand-registered Monarch grammar below (Monaco's language definitions have
// no JSON grammar; JSON's stock highlighting ships only with the language
// service).
import setupSource from "./monacoSetup.ts?raw"

describe("monacoSetup ships no language-service workers", () => {
  it("does not import the JSON language service or its worker", () => {
    expect(setupSource).not.toContain("vs/language/json/monaco.contribution")
    expect(setupSource).not.toContain("languages/features/json")
    expect(setupSource).not.toContain("json.worker")
  })

  it("imports the bare editor, its features and the Monarch definitions, never the language services", () => {
    const imports = setupSource.match(/^import [^\n]*"monaco-editor[^"]*"/gm) ?? []
    expect(imports).toEqual([
      'import * as monaco from "monaco-editor/editor"',
      'import "monaco-editor/features/register.all"',
      'import "monaco-editor/languages/definitions/register.all"',
      'import editorWorker from "monaco-editor/editor/editor.worker?worker"',
    ])
  })

  it("registers a Monarch JSON grammar so .json files keep coloring", () => {
    expect(setupSource).toContain('monaco.languages.register({ id: "json"')
    expect(setupSource).toContain('setMonarchTokensProvider("json"')
  })

  it("wires only the editor worker", () => {
    const workerImports = setupSource.match(/from "[^"]*\?worker"/g) ?? []
    expect(workerImports).toEqual([
      'from "monaco-editor/editor/editor.worker?worker"',
    ])
  })
})
