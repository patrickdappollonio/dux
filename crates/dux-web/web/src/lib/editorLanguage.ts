// The editor's LANGUAGE picker, and the language resolution behind it.
//
// Monaco infers a file's language from its URI, and that inference stays the
// default: dux ships no filename-to-language table of its own, so a `.lock`
// file still opens as plain text. What this adds is a way to say otherwise
// for one open file, which is the whole point of a picker: the file dux
// guessed wrong about is by definition the one no table would have covered.
//
// Everything here is PURE and takes the registered-language list as an
// argument. Monaco cannot load under vitest (see lib/pathExt.ts), so nothing
// in this module may import it; the component reads
// `monaco.languages.getLanguages()` at runtime and passes the result in.

import { extensionForPath, fileNameForPath } from "@/lib/pathExt"

/// The shape of one entry in Monaco's language registry, narrowed to the
/// fields dux reads. Structurally satisfied by monaco's own
/// `ILanguageExtensionPoint`.
export interface RegisteredLanguage {
  id: string
  aliases?: readonly string[] | null
  extensions?: readonly string[] | null
  filenames?: readonly string[] | null
}

/// One row in the picker.
export interface LanguageChoice {
  id: string
  label: string
}

/// Monaco's id for "no language", and what dux calls it.
///
/// The registry's own alias for it is "Plain Text". dux says "Plain text"
/// instead, in the list AND on the trigger, because the trigger also has to
/// name the state where no language was inferred at all, and a menu row
/// reading "Plain Text" beside a trigger reading "Plain text" is the kind of
/// small disagreement that makes a control look broken.
export const PLAIN_TEXT_ID = "plaintext"
export const PLAIN_TEXT_LABEL = "Plain text"

/// The display label for a registered language: its first alias, or its id
/// when it has none. Monaco's aliases are the human spellings ("TypeScript"
/// for `typescript`, "Shell" for `shell`); the id is a fallback for a
/// hand-registered grammar that shipped without one.
export function languageLabel(lang: RegisteredLanguage): string {
  if (lang.id === PLAIN_TEXT_ID) return PLAIN_TEXT_LABEL
  const alias = lang.aliases?.find((a) => a.trim() !== "")
  return alias ?? lang.id
}

/// The picker's rows: every registered language, sorted by label.
///
/// Case-insensitive, because a registry holding "bat" beside "BibTeX" sorts
/// nonsensically under a plain codepoint comparison, and numeric so a "C++"
/// does not land oddly. Ties break on id so the order is total and a render
/// cannot reshuffle between two languages sharing a label.
export function languagePickerEntries(
  langs: readonly RegisteredLanguage[],
): LanguageChoice[] {
  return langs
    .map((lang) => ({ id: lang.id, label: languageLabel(lang) }))
    .sort(
      (a, b) =>
        a.label.localeCompare(b.label, undefined, {
          sensitivity: "base",
          numeric: true,
        }) || a.id.localeCompare(b.id),
    )
}

/// The language Monaco WOULD infer for a path from the registered grammars:
/// an extension match first, then a whole-filename match ("Makefile",
/// "Dockerfile"). `undefined` when nothing claims it, which Monaco renders as
/// plain text.
///
/// This is the same walk `monacoSetup.monacoLanguageForPath` performs, with
/// the registry injected rather than imported, which is what makes it
/// testable; that function now delegates here so the two cannot drift.
export function inferredLanguageId(
  path: string,
  langs: readonly RegisteredLanguage[],
): string | undefined {
  const ext = extensionForPath(path)
  const file = fileNameForPath(path)
  for (const lang of langs) {
    if (ext && lang.extensions?.some((e) => e.toLowerCase() === ext)) {
      return lang.id
    }
    if (lang.filenames?.some((f) => f.toLowerCase() === file)) {
      return lang.id
    }
  }
  return undefined
}

/// The language actually in force for a path: the user's override when there
/// is one, otherwise `undefined`, which means LET MONACO INFER.
///
/// Returning `undefined` rather than the inferred id is deliberate: it is
/// what the `language` prop is given, and passing nothing leaves the
/// inference to Monaco's own URI handling instead of dux re-deciding it every
/// render off a registry snapshot that may have been read before a grammar
/// finished registering.
export function languageOverrideFor(
  overrides: ReadonlyMap<string, string>,
  path: string | null,
): string | undefined {
  if (path === null) return undefined
  return overrides.get(path)
}

/// What the trigger says: the label of the language in force, override or
/// inferred, falling back to plain text when nothing claims the file.
export function effectiveLanguageLabel(
  overrides: ReadonlyMap<string, string>,
  path: string | null,
  langs: readonly RegisteredLanguage[],
): string {
  const id =
    languageOverrideFor(overrides, path) ??
    (path === null ? undefined : inferredLanguageId(path, langs))
  if (id === undefined) return PLAIN_TEXT_LABEL
  const lang = langs.find((l) => l.id === id)
  // An override naming a language the registry does not have is still shown,
  // by id: it is what was picked, and hiding it would make the control look
  // as though the pick had not taken.
  return lang === undefined ? id : languageLabel(lang)
}

/// Set or clear one path's override. `null` is the picker's "Auto" row: it
/// REMOVES the entry rather than storing a sentinel, so "no override" has one
/// representation and `languageOverrideFor` needs no second check.
export function withLanguageOverride(
  overrides: ReadonlyMap<string, string>,
  path: string,
  id: string | null,
): Map<string, string> {
  const next = new Map(overrides)
  if (id === null) next.delete(path)
  else next.set(path, id)
  return next
}
