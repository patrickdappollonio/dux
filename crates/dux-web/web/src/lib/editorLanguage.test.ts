import { describe, expect, it } from "vitest"

import {
  effectiveLanguageLabel,
  inferredLanguageId,
  languageLabel,
  languageOverrideFor,
  languagePickerEntries,
  PLAIN_TEXT_ID,
  PLAIN_TEXT_LABEL,
  withLanguageOverride,
} from "./editorLanguage"
import type { RegisteredLanguage } from "./editorLanguage"

// A stand-in for Monaco's registry. The real one is read at runtime and
// passed in, precisely so these tests never need Monaco, which cannot load
// under vitest at all.
const REGISTRY: RegisteredLanguage[] = [
  { id: PLAIN_TEXT_ID, aliases: ["Plain Text", "text"], extensions: [".txt"] },
  { id: "typescript", aliases: ["TypeScript", "ts"], extensions: [".ts"] },
  { id: "rust", aliases: ["Rust", "rs"], extensions: [".rs"] },
  { id: "toml", aliases: ["TOML"], extensions: [".toml"] },
  { id: "shell", aliases: ["Shell", "sh"], extensions: [".sh"] },
  { id: "dockerfile", aliases: ["Dockerfile"], filenames: ["Dockerfile"] },
  // A hand-registered grammar that shipped without an alias.
  { id: "duckscript" },
]

describe("languageLabel", () => {
  it("uses the first alias, which is the human spelling", () => {
    expect(languageLabel(REGISTRY[1])).toBe("TypeScript")
  })

  it("falls back to the id when a grammar registered no alias", () => {
    expect(languageLabel({ id: "duckscript" })).toBe("duckscript")
    expect(languageLabel({ id: "duckscript", aliases: [] })).toBe("duckscript")
    expect(languageLabel({ id: "duckscript", aliases: ["  "] })).toBe(
      "duckscript",
    )
  })

  it("says 'Plain text' rather than the registry's 'Plain Text'", () => {
    // The trigger has to name the no-language state too, and a row reading
    // "Plain Text" beside a trigger reading "Plain text" looks like a bug.
    expect(languageLabel(REGISTRY[0])).toBe(PLAIN_TEXT_LABEL)
  })
})

describe("languagePickerEntries", () => {
  it("lists every registered language once", () => {
    const entries = languagePickerEntries(REGISTRY)
    expect(entries.length).toBe(REGISTRY.length)
    expect(new Set(entries.map((e) => e.id)).size).toBe(REGISTRY.length)
  })

  it("sorts by label, case-insensitively", () => {
    // A codepoint sort would put every capitalised alias before "duckscript".
    const labels = languagePickerEntries(REGISTRY).map((e) => e.label)
    expect(labels).toEqual([
      "Dockerfile",
      "duckscript",
      "Plain text",
      "Rust",
      "Shell",
      "TOML",
      "TypeScript",
    ])
  })

  it("breaks a label tie on id, so the order is total", () => {
    const entries = languagePickerEntries([
      { id: "b", aliases: ["Same"] },
      { id: "a", aliases: ["Same"] },
    ])
    expect(entries.map((e) => e.id)).toEqual(["a", "b"])
  })

  it("handles an empty registry (nothing has been read yet)", () => {
    expect(languagePickerEntries([])).toEqual([])
  })
})

describe("inferredLanguageId", () => {
  it("matches on extension", () => {
    expect(inferredLanguageId("src/main.rs", REGISTRY)).toBe("rust")
    expect(inferredLanguageId("a/b/config.TOML", REGISTRY)).toBe("toml")
  })

  it("matches a whole filename when no extension claims it", () => {
    expect(inferredLanguageId("docker/Dockerfile", REGISTRY)).toBe("dockerfile")
  })

  it("is undefined when nothing claims the file, which Monaco renders as plain text", () => {
    // The `.lock` case the picker exists for: no table is added, the picker
    // is the answer.
    expect(inferredLanguageId("Cargo.lock", REGISTRY)).toBeUndefined()
    expect(inferredLanguageId("LICENSE", REGISTRY)).toBeUndefined()
  })
})

describe("languageOverrideFor", () => {
  it("is the override when one is set for that exact path", () => {
    const overrides = new Map([["Cargo.lock", "toml"]])
    expect(languageOverrideFor(overrides, "Cargo.lock")).toBe("toml")
  })

  it("is undefined for an unoverridden path, which means let Monaco infer", () => {
    const overrides = new Map([["Cargo.lock", "toml"]])
    expect(languageOverrideFor(overrides, "src/main.rs")).toBeUndefined()
    expect(languageOverrideFor(overrides, null)).toBeUndefined()
  })
})

describe("effectiveLanguageLabel", () => {
  const none = new Map<string, string>()

  it("names the inferred language when there is no override", () => {
    expect(effectiveLanguageLabel(none, "src/main.rs", REGISTRY)).toBe("Rust")
  })

  it("names plain text when nothing claims the file", () => {
    expect(effectiveLanguageLabel(none, "Cargo.lock", REGISTRY)).toBe(
      PLAIN_TEXT_LABEL,
    )
  })

  it("names the override, which is the whole point of the control", () => {
    const overrides = new Map([["Cargo.lock", "toml"]])
    expect(effectiveLanguageLabel(overrides, "Cargo.lock", REGISTRY)).toBe(
      "TOML",
    )
  })

  it("names plain text when there is no file open", () => {
    expect(effectiveLanguageLabel(none, null, REGISTRY)).toBe(PLAIN_TEXT_LABEL)
  })

  it("shows an override the registry does not know by its raw id", () => {
    // Hiding it would make the control look as though the pick had not taken.
    const overrides = new Map([["a.txt", "cobol"]])
    expect(effectiveLanguageLabel(overrides, "a.txt", REGISTRY)).toBe("cobol")
  })
})

describe("withLanguageOverride", () => {
  it("sets one path's override without touching the others", () => {
    const before = new Map([["a.txt", "toml"]])
    const after = withLanguageOverride(before, "b.txt", "rust")
    expect(after.get("a.txt")).toBe("toml")
    expect(after.get("b.txt")).toBe("rust")
    // The input is not mutated: React state.
    expect(before.has("b.txt")).toBe(false)
  })

  it("replaces an existing override", () => {
    const after = withLanguageOverride(new Map([["a.txt", "toml"]]), "a.txt", "rust")
    expect(after.get("a.txt")).toBe("rust")
  })

  it("Auto REMOVES the entry rather than storing a sentinel", () => {
    // So "no override" has exactly one representation.
    const after = withLanguageOverride(new Map([["a.txt", "toml"]]), "a.txt", null)
    expect(after.has("a.txt")).toBe(false)
    expect(languageOverrideFor(after, "a.txt")).toBeUndefined()
  })
})
