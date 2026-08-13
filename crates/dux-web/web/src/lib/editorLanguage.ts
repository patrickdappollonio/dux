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

/// What the live Monaco model's language must be set to after the `language`
/// prop changed, or `null` when nothing needs doing.
///
/// This exists because of a MEASURED gap in @monaco-editor/react 4.7.0: its
/// language effect reads `let m = editor.getModel(); m && s && setModelLanguage(m, s)`,
/// so a prop going from a language id to `undefined` sets nothing at all and
/// the model keeps the language the user is trying to clear. Picking "Auto"
/// then changed only the trigger's label, which is worse than doing nothing.
///
/// So the only case that needs help is the defined-to-undefined transition on
/// the SAME file. A defined value is the wrapper's job, and a PATH change
/// swaps the model entirely (the wrapper creates it with an empty language and
/// Monaco does its own, richer inference, which also reads the first line), so
/// stepping in there would replace a better answer with a worse one.
///
/// The nuance that follows, accepted rather than fought: Monaco's first-open
/// inference reads shebangs, and this walk only knows extensions and whole
/// filenames. An explicit Auto on an extensionless shell script therefore lands
/// on plain text where the first open showed Shell. Reopening the file restores
/// the richer guess.
export function autoRevertLanguageId(
  prev: { language?: string; path: string },
  next: { language?: string; path: string },
  langs: readonly RegisteredLanguage[],
): string | null {
  if (prev.path !== next.path) return null
  if (prev.language === undefined || next.language !== undefined) return null
  return inferredLanguageId(next.path, langs) ?? PLAIN_TEXT_ID
}

/// Retarget every override that lives under `from` onto `to`, mirroring the
/// tab retargeting in `editorTabs.renameTabPaths` (an exact path match, or a
/// path inside a renamed DIRECTORY).
///
/// Without this a rename silently reverted the language of a file the user had
/// just corrected, and left the old key behind for an unrelated file that
/// later took that path to inherit.
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

/// Drop every override whose file is no longer open.
///
/// The overrides are session-lived, but "session" has to mean the tab: the docs
/// promise an override lasts until you close the file, and without this a
/// closed-and-reopened path came back still overridden. Returns the SAME map
/// when nothing needs dropping, so a caller can hand it straight to a React
/// setState and have the render bail out.
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
