// The editor's language picker, and the language resolution behind it.
//
// Monaco's URI inference stays the default: dux ships no filename-to-language
// table of its own, so a `.lock` file still opens as plain text, and the picker
// overrides that for one open file. Everything here is pure and takes the
// registered-language list as an argument, because Monaco cannot load under
// vitest (see lib/pathExt.ts) and nothing in this module may import it.

import { extensionForPath, fileNameForPath } from "@/lib/pathExt"

/// One entry in Monaco's language registry, narrowed to the fields dux reads.
/// Structurally satisfied by monaco's own `ILanguageExtensionPoint`.
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

/// Monaco's id for "no language", and what dux calls it. The registry's own
/// alias is "Plain Text"; dux says "Plain text" in the list and on the trigger,
/// which must also name the state where no language was inferred at all.
export const PLAIN_TEXT_ID = "plaintext"
export const PLAIN_TEXT_LABEL = "Plain text"

/// The display label for a registered language: its first alias (Monaco's human
/// spelling), or its id for a grammar registered without one.
export function languageLabel(lang: RegisteredLanguage): string {
  if (lang.id === PLAIN_TEXT_ID) return PLAIN_TEXT_LABEL
  const alias = lang.aliases?.find((a) => a.trim() !== "")
  return alias ?? lang.id
}

/// The picker's rows: every registered language, sorted by label,
/// case-insensitively and numerically, since a registry holding "bat" beside
/// "BibTeX" sorts nonsensically by codepoint. Ties break on id so the order is
/// total and a render cannot reshuffle two languages sharing a label.
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

/// The language Monaco would infer for a path from the registered grammars: an
/// extension match first, then a whole-filename match. `undefined` when nothing
/// claims it, which Monaco renders as plain text. `monacoLanguageForPath`
/// delegates here, with the registry injected rather than imported.
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

/// The language in force for a path: the user's override, or `undefined`, which
/// means let Monaco infer. `undefined` rather than the inferred id is what the
/// `language` prop wants, leaving inference to Monaco's URI handling instead of
/// re-deciding it every render off a registry snapshot that may predate a
/// grammar finishing registration.
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
  // An override naming a language the registry does not have is still shown, by
  // id: hiding it would make the control look as though the pick had not taken.
  return lang === undefined ? id : languageLabel(lang)
}

/// What the live Monaco model's language must be set to after the `language`
/// prop changed, or `null` when nothing needs doing.
///
/// @monaco-editor/react 4.7.0's language effect skips an undefined value, so a
/// prop going from a language id back to `undefined` leaves the model on the
/// language the user is clearing; only that defined-to-undefined transition on
/// the same file needs help. A defined value is the wrapper's job, and a path
/// change swaps the model, where Monaco's own inference reads the first line and
/// is the better answer. The cost: this walk knows only extensions and whole
/// filenames, so an explicit Auto on an extensionless shell script lands on
/// plain text until the file is reopened.
export function autoRevertLanguageId(
  prev: { language?: string; path: string },
  next: { language?: string; path: string },
  langs: readonly RegisteredLanguage[],
): string | null {
  if (prev.path !== next.path) return null
  if (prev.language === undefined || next.language !== undefined) return null
  return inferredLanguageId(next.path, langs) ?? PLAIN_TEXT_ID
}

/// Retarget every override under `from` onto `to`, mirroring
/// `editorTabs.renameTabPaths`: an exact path match, or a path inside a renamed
/// directory. Without it a rename reverts the language the user just corrected
/// and leaves the old key for a later file at that path to inherit.
export function retargetLanguageOverrides(
  overrides: ReadonlyMap<string, string>,
  from: string,
  to: string,
): Map<string, string> {
  const next = new Map<string, string>()
  for (const [path, id] of overrides) {
    if (path === from) next.set(to, id)
    else if (path.startsWith(`${from}/`)) next.set(to + path.slice(from.length), id)
    else next.set(path, id)
  }
  return next
}

/// Drop every override whose file is no longer open: an override lasts until
/// the file is closed, so a reopened path must come back un-overridden. Returns
/// the same map when nothing needs dropping, so a caller can hand it straight to
/// a React setState and have the render bail out.
export function pruneLanguageOverrides(
  overrides: Map<string, string>,
  openPaths: ReadonlySet<string>,
): Map<string, string> {
  let stale = false
  for (const path of overrides.keys()) {
    if (!openPaths.has(path)) {
      stale = true
      break
    }
  }
  if (!stale) return overrides
  const next = new Map<string, string>()
  for (const [path, id] of overrides) {
    if (openPaths.has(path)) next.set(path, id)
  }
  return next
}

/// Set or clear one path's override. `null` is the picker's "Auto" row and
/// removes the entry rather than storing a sentinel, so "no override" has one
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
