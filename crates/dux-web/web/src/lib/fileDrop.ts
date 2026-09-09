// Pure helpers for a file drop onto a terminal or agent pane. dux saves the file
// and pastes its path: no agent CLI reads a file from its input stream.

/// How many file names one toast spells out before it points at the folder
/// instead.
export const MAX_NAMED_FILES = 5

/// Whether a drag carries files from outside the browser. `types` is all a
/// dragover can read, and an in-app drag carries no `"Files"` entry.
export function dragCarriesFiles(
  types: readonly string[] | undefined,
): boolean {
  return Array.from(types ?? []).includes("Files")
}

/// One saved file. The folder belongs per file rather than to the drop: uploads
/// are sequential and a terminal's directory can change between them.
export type SavedFile = {
  requestedName: string
  savedName: string
  /// The absolute path, which is what the user needs when the path was not sent.
  path: string
  /// The folder, already shortened with `~` by the server (which is the machine
  /// whose home directory it is).
  folderLabel: string
}

/// What became of one dropped file. The toast reads these in the order the files
/// were dropped, which is also the order their paths are sent.
export type DropOutcome =
  /// Saved, and the path was written to an open socket we own. Nothing
  /// acknowledges a PTY write, so this means sent, never that it arrived.
  | ({ kind: "sent" } & SavedFile)
  /// Saved, but the path was not sent: we do not hold input, or the socket was
  /// closed. Carries the full path so the user can reach the file by hand.
  | ({ kind: "saved-not-sent"; reason: string } & SavedFile)
  /// Never saved. The reason is the server's own words, not a generic one.
  | { kind: "refused"; requestedName: string; reason: string }

/// Does the server's own text already advise a retry? Matched within one
/// sentence so "try ... again" cannot be assembled out of two unrelated ones.
const ADVISES_RETRY = /\btry\b[^.!?]*\bagain\b/i

/// Why an upload was refused, in words the user can act on, from the HTTP status
/// and whatever the server said. The server's own words win where it has any;
/// this only adds what they do not already say.
export function dropRefusalReason(status: number, detail: string): string {
  const said = detail.trim()
  // 0 is the transport failure `uploadDroppedFile` reports when `fetch` rejected.
  if (status === 0) {
    return said || "the server could not be reached"
  }
  if (status === 503) {
    // The server-side wait is bounded, so this means no upload slot came free.
    if (!said) {
      return "the server was busy with other uploads, so it was not saved; try the drop again in a moment"
    }
    // dux's own 503 body already advises a retry; the tail would say it twice.
    if (ADVISES_RETRY.test(said)) return asClause(said)
    // A finished sentence cannot take a comma-spliced clause, so the tail becomes
    // its own sentence and only a fragment gets the comma.
    if (/[.!?]$/.test(said)) {
      return `${said} It was not saved; try the drop again in a moment`
    }
    return `${asClause(said)}, so it was not saved; try the drop again in a moment`
  }
  return said || `the server refused the upload (${status})`
}

/// How the destination is described, which is a property of the whole drop
/// rather than of one file.
export type DropContext = {
  /// Only for the wording. Both kinds report the real folder the server sent for
  /// each saved file; the kind decides the fallback name when the server sent
  /// none, and whether several folders in one drop are explained.
  kind: "agent" | "terminal"
  /// Which verb the report uses. `"sent"` (the default) means written to the
  /// PTY; `"draft"` means spliced into the compose bar, where nothing has gone
  /// to the agent yet.
  delivery?: "sent" | "draft"
  /// Set only when the batch is one long text paste dux turned into a document,
  /// and then it is that paste's character count, which leads the report.
  pastedTextChars?: number
}

/// "sent its path" / "added its path to your message", for one file.
function deliveredOne(ctx: DropContext): string {
  return ctx.delivery === "draft"
    ? "added its path to your message"
    : "sent its path"
}

/// The same, for several.
function deliveredMany(ctx: DropContext): string {
  return ctx.delivery === "draft"
    ? "added their paths to your message"
    : "sent their paths"
}

/// The negative form, used by the stranded-file rung.
function notDelivered(ctx: DropContext): string {
  return ctx.delivery === "draft" ? "not added" : "not sent"
}

export type DropToast = {
  tone: "success" | "warning" | "error"
  message: string
  /// Whether this report waits for the user instead of for a clock. The bar is
  /// deliberately high (see `NotifyOptions.sticky` in `lib/notify.ts`): recovery
  /// happens outside the toast, or something may have been lost.
  sticky: boolean
}

/// The form a dropped file's path takes in the prompt. These are the exact
/// strings `dux_core::config::WebDragDropPaste` publishes in `DropPasteView`.
export type DragDropPasteForm =
  | "bare"
  | "single_quoted"
  | "double_quoted"
  | "backslash_escaped"

const DRAG_DROP_PASTE_FORMS: readonly DragDropPasteForm[] = [
  "bare",
  "single_quoted",
  "double_quoted",
  "backslash_escaped",
]

/// One pane's paste inputs, mirroring `dux_core::viewmodel::DropPasteView` field
/// names. The two resolve together, never separately: they answer the same
/// question, which CLI is on the other end of this paste.
export type DropPasteProfile = {
  /// One of the `DragDropPasteForm` names, normalized server-side. Typed as a
  /// plain string because it arrives off the wire, and validated on use.
  form: string
  /// The file name of the command being run, not the provider's block name: a
  /// provider's name is free text, so `[providers.codex] command =
  /// "something-else"` is not Codex.
  command_name: string
}

/// `bootstrap.provider_drop_paste`, keyed by provider name: what config says now,
/// `undefined` on an older server. The fallback for a pane with no live process,
/// refreshed by `config.changed`, the event that can change it.
export type ConfiguredDropPaste = Record<string, DropPasteProfile> | undefined

/// What a drop is landing on. An agent tab has a launched profile (`undefined`
/// when nothing is live) and a configured fallback; a terminal has neither.
export type DropPasteTarget =
  | {
      kind: "agent"
      /// `AgentTabView.drop_paste`: what this tab's live process launched with.
      launched: DropPasteProfile | undefined
      /// The tab's effective provider name, used only to look up the configured
      /// fallback when nothing is live.
      provider: string | undefined
    }
  | { kind: "terminal" }

/// The form a plain terminal always gets, whatever the provider settings say.
/// dux permits `$`, a backtick, a space, a semicolon, a quote and parentheses in
/// a path, and a shell would split or expand a bare one; inside POSIX single
/// quotes nothing is special. A non-POSIX `[terminal] command` (PowerShell, fish)
/// gets no form of its own until someone has measured one.
export const TERMINAL_PASTE_FORM: DragDropPasteForm = "single_quoted"

/// The one profile that applies to a pane, or `undefined` when nothing names it.
/// The tab's launched profile wins over config, so a config edit takes effect at
/// that tab's next launch. A terminal has neither; see `TERMINAL_PASTE_FORM`.
function dropPasteProfileFor(
  configured: ConfiguredDropPaste,
  target: DropPasteTarget,
): DropPasteProfile | undefined {
  if (target.kind === "terminal") return undefined
  if (target.launched !== undefined) return target.launched
  if (target.provider === undefined) return undefined
  return configured?.[target.provider]
}

/// Which form the pane being dropped on needs. A terminal is decided first; an
/// unnamed provider, an older server or a form name this client does not know
/// falls back to `bare` rather than being trusted into somebody's prompt.
export function dragDropPasteFormFor(
  configured: ConfiguredDropPaste,
  target: DropPasteTarget,
): DragDropPasteForm {
  if (target.kind === "terminal") return TERMINAL_PASTE_FORM
  const name = dropPasteProfileFor(configured, target)?.form
  return DRAG_DROP_PASTE_FORMS.find((f) => f === name) ?? "bare"
}

/// How many characters of pasted text a CLI still reads as a possible file path,
/// keyed by the command's file name: Codex's composer files a paste over its own
/// 1000 character threshold away as large content before it looks for a path.
///
/// Keyed by the command, not by form (which a terminal shares without sharing
/// the limit) and not by provider name (free text that need not match its
/// command). The server publishes the command's file name, so a full path
/// resolves like a bare one. A command absent from this table has no limit, and
/// this is a measurement of a third-party CLI rather than a setting: add an
/// entry only once someone has measured it.
const COMMAND_ATTACHMENT_CHAR_LIMITS: Record<string, number> = {
  codex: 1000,
}

/// The character limit that applies to a drop target, or `null` when none does.
/// A terminal has none: a shell has no composer to file a long paste away.
export function attachmentCharLimitFor(
  configured: ConfiguredDropPaste,
  target: DropPasteTarget,
): number | null {
  const command = dropPasteProfileFor(configured, target)?.command_name
  if (command === undefined) return null
  return COMMAND_ATTACHMENT_CHAR_LIMITS[command] ?? null
}

/// What a pane needs to turn one saved file into one paste. The two are resolved
/// together and never derived from each other; see `DropPasteProfile`.
export type DropPastePlan = {
  form: DragDropPasteForm
  /// `null` means unlimited, not "zero".
  charLimit: number | null
}

/// Resolve both halves for one drop target, from the one profile that applies to
/// it, so they can never come from two different CLIs.
export function dragDropPasteFor(
  configured: ConfiguredDropPaste,
  target: DropPasteTarget,
): DropPastePlan {
  return {
    form: dragDropPasteFormFor(configured, target),
    charLimit: attachmentCharLimitFor(configured, target),
  }
}

/// Whether this payload is too long for the receiving CLI to read as a file path.
/// Takes the whole payload, since the quoting and trailing space are pasted too,
/// and counts characters rather than UTF-16 units, because that is what the CLI
/// counts. A `null` limit refuses nothing.
export function pasteExceedsAttachmentLimit(
  payload: string,
  limit: number | null,
): boolean {
  // Strictly greater: a payload of exactly the limit still gets looked at.
  return limit !== null && [...payload].length > limit
}

/// Why a saved file's path was held back, in the words the stranded-file toast
/// shows after "the path was not sent: ". That toast still reports the path in
/// full, which is how the user hands it to the agent themselves.
export function tooLongToAttachReason(limit: number): string {
  return (
    `the path is longer than this agent reads as a file path ` +
    `(${limit} characters, counting the quoting dux adds), so it would have been ` +
    `taken as ordinary pasted text rather than attached`
  )
}

/// The characters `backslashEscaped` protects: whitespace, the quoting and
/// expansion characters, the shell's operators, and the glob characters. ASCII
/// only on purpose: escaping every CJK codepoint would make the prompt
/// unreadable for the users most likely to have one, for no lexical gain.
const SHELL_SIGNIFICANT = /[\s"#$&'()*;<>?[\\\]`{|}~]/g

/// Wrap in single quotes, closing and reopening around each embedded apostrophe.
/// Inside POSIX single quotes nothing else is special, so nothing else is
/// escaped, and leaving the quotes is the only way to include an apostrophe.
function singleQuoted(path: string): string {
  return `'${path.replaceAll("'", `'\\''`)}'`
}

/// Wrap in double quotes, escaping all four characters a double-quoted string
/// gives meaning to: `"`, `\`, `$` and a backtick. Shell lexing removes the
/// backslash again, so escaping all four is lossless and stays safe if the paste
/// ever reaches something that evaluates what it reads rather than lexing it.
function doubleQuoted(path: string): string {
  return `"${path.replaceAll(/[\\"$`]/g, (c) => `\\${c}`)}"`
}

/// No quotes; escape each shell-significant character on its own.
function backslashEscaped(path: string): string {
  return path.replace(SHELL_SIGNIFICANT, (c) => `\\${c}`)
}

/// What to paste for one saved file: the path in the given form, one trailing
/// space, and no newline, which would submit a half-written prompt. One file per
/// paste, because these CLIs attach only when the whole paste is that one path.
///
/// The measured per-CLI table is on `dux_core::config::WebDragDropPaste`. Claude
/// Code and OpenCode read the whole string, so `bare` is right, and single
/// quoting breaks Claude Code on a path holding an apostrophe; Codex lexes with
/// POSIX rules and accepts one token only, so it needs `single_quoted`. Known
/// limitations: a path containing a backslash is mangled by Claude Code and
/// OpenCode in every form, and OpenCode strips quotes off both ends rather than
/// one matching pair. Length is answered by `pasteExceedsAttachmentLimit`.
export function pastePayload(path: string, form: DragDropPasteForm): string {
  switch (form) {
    case "single_quoted":
      return `${singleQuoted(path)} `
    case "double_quoted":
      return `${doubleQuoted(path)} `
    case "backslash_escaped":
      return `${backslashEscaped(path)} `
    case "bare":
      return `${path} `
  }
}

/// Every distinct folder the saved files landed in, in the order they were hit.
function foldersOf(saved: SavedFile[]): string[] {
  return [...new Set(saved.map((s) => s.folderLabel).filter(Boolean))]
}

/// One phrase for where the drop went, or empty when the files landed in more
/// than one folder and no single phrase is true. Callers then use
/// `folderBreakdown`.
function folderPhrase(saved: SavedFile[], ctx: DropContext): string {
  const folders = foldersOf(saved)
  if (folders.length === 1) return folders[0]
  // An empty label from the server still needs something true to say.
  if (folders.length === 0) {
    return ctx.kind === "agent"
      ? "the agent's upload folder"
      : "the terminal's folder"
  }
  return ""
}

/// The per-folder listing used when one phrase cannot cover the drop, grouped by
/// folder so three files in two folders read as two clauses instead of three.
function folderBreakdown(saved: SavedFile[], ctx: DropContext): string {
  if (folderPhrase(saved, ctx) !== "") return ""
  const order: string[] = []
  const byFolder = new Map<string, string[]>()
  for (const s of saved) {
    const names = byFolder.get(s.folderLabel)
    if (names) names.push(s.savedName)
    else {
      byFolder.set(s.folderLabel, [s.savedName])
      order.push(s.folderLabel)
    }
  }
  const clauses = order.map((folder) => {
    const names = byFolder.get(folder) ?? []
    const listed =
      names.length > MAX_NAMED_FILES
        ? `${names.length} files`
        : names.join(" and ")
    return `${listed} to ${folder}`
  })
  // Only a terminal can scatter a drop across folders, so only it has a why.
  const why =
    ctx.kind === "terminal" ? "A terminal moves, so they" : "They"
  return ` ${why} did not all land together: ${clauses.join(", ")}.`
}

/// `to <somewhere>` when one phrase covers the drop, and nothing when it does
/// not, because `folderBreakdown` then says it properly.
function toPhrase(saved: SavedFile[], ctx: DropContext): string {
  const where = folderPhrase(saved, ctx)
  return where === "" ? "" : ` to ${where}`
}

/// The stranded files that share a reason, grouped, in the order the reasons
/// were first hit. Uploads are sequential, so one drop can genuinely strand
/// files for two different reasons and neither may be dropped.
function strandedByReason(
  notSent: (SavedFile & { reason: string })[],
): { reason: string; files: SavedFile[] }[] {
  const groups: { reason: string; files: SavedFile[] }[] = []
  for (const n of notSent) {
    const group = groups.find((g) => g.reason === n.reason)
    if (group) group.files.push(n)
    else groups.push({ reason: n.reason, files: [n] })
  }
  return groups
}

/// Stranded files named with their full paths, since the user has to find them
/// by hand. Capped, with the remainder counted rather than dropped silently.
function strandedList(files: SavedFile[]): string {
  const named = files
    .slice(0, MAX_NAMED_FILES)
    .map((f) => `${f.savedName} (${f.path})`)
    .join(", ")
  return files.length > MAX_NAMED_FILES
    ? `${named} and ${files.length - MAX_NAMED_FILES} more`
    : named
}

/// End a sentence with exactly one terminator. A server's reason is already a
/// whole sentence and a reason dux writes itself is not, and both go through the
/// same templates.
export function endSentence(text: string): string {
  const trimmed = text.trimEnd()
  if (trimmed === "") return trimmed
  return /[.!?]$/.test(trimmed) ? trimmed : `${trimmed}.`
}

/// Drop a trailing terminator from a reason that is about to be embedded inside
/// a clause or a pair of parentheses, where a full stop would land mid-sentence.
export function asClause(text: string): string {
  return text.trimEnd().replace(/\.+$/, "")
}

/// The refused files, named with their reasons. Deliberately does not end in a
/// period: one caller continues the sentence afterwards.
function reasonList(items: { requestedName: string; reason: string }[]): string {
  if (items.length > MAX_NAMED_FILES) {
    return `${items.length} files were refused; the first was ${items[0].requestedName} (${asClause(items[0].reason)})`
  }
  return items.map((r) => `${r.requestedName} (${asClause(r.reason)})`).join(", ")
}

/// The renamed-file note, applied to every saved file at every rung: a file that
/// was renamed and whose path never went out is one the user must find by hand
/// under a name they were never told.
function renameNote(saved: SavedFile[], ctx: DropContext): string {
  const renamed = saved.filter((s) => s.requestedName !== s.savedName)
  if (renamed.length === 0) return ""
  // Named, never counted: a count does not say what the file is now called.
  if (renamed.length > MAX_NAMED_FILES) {
    const where = folderPhrase(renamed, ctx)
    return ` ${renamed.length} already existed and were saved under new names, which are listed in ${where === "" ? "the folders above" : where}.`
  }
  const pairs = renamed
    .map((r) => `${r.requestedName} was saved as ${r.savedName}`)
    .join(", ")
  return ` ${pairs}, so nothing was overwritten.`
}

/// The one toast for a whole drop, so a handful of files cannot bury the screen.
/// The rung is the first that applies, so a bad outcome is never reported good:
///
///   1. nothing saved               -> error
///   2. anything saved but not sent -> warning, naming those files' full paths
///   3. anything refused            -> warning
///   4. otherwise                   -> success
///
/// Every rung with a saved file also says what a renamed file is now called, and
/// which folder each file went to when they did not all go to the same one.
export function dropToastFor(
  outcomes: DropOutcome[],
  ctx: DropContext,
): DropToast {
  const report = savedFilesToast(outcomes, ctx)
  if (ctx.pastedTextChars === undefined) return report
  const anySaved = outcomes.some(
    (o) => o.kind === "sent" || o.kind === "saved-not-sent",
  )
  return {
    ...report,
    // Sticky: dux cancelled the paste, so a text paste that saved nothing
    // survives only on the clipboard and the recovery line is the way back.
    sticky: report.sticky || !anySaved,
    message:
      pastedTextLead(ctx.pastedTextChars, anySaved, ctx) +
      report.message +
      PASTED_TEXT_RECOVERY,
  }
}

/// The sentence in front of every rung when the "files" were one long text
/// paste. It states the size, which is what the user can act on. The verb
/// follows whether anything was saved and the destination follows
/// `ctx.delivery`, or the lead-in contradicts the rung printed after it.
function pastedTextLead(
  chars: number,
  saved: boolean,
  ctx: DropContext,
): string {
  const verb = saved ? "saved" : "tried to save"
  const instead =
    ctx.delivery === "draft"
      ? "putting the text in your message"
      : "typing it into the agent"
  return `That paste was ${chars} characters, so dux ${verb} it as a file rather than ${instead}. `
}

/// The way back, appended to every rung of a filed-away text paste: dux cancels
/// the paste event, so on a failing rung this is the whole recovery. The chord
/// names both platforms rather than sniffing one, matching the docs.
const PASTED_TEXT_RECOVERY =
  ' Your text is still on the clipboard: press Ctrl+Shift+v (Cmd+Shift+v on a Mac) to paste it as text, or change when this happens under "Save long pastes as a file" in Preferences.'

function savedFilesToast(
  outcomes: DropOutcome[],
  ctx: DropContext,
): DropToast {
  const sent = outcomes.filter((o) => o.kind === "sent")
  const notSent = outcomes.filter((o) => o.kind === "saved-not-sent")
  const refused = outcomes.filter((o) => o.kind === "refused")
  const savedFiles: SavedFile[] = [...sent, ...notSent]

  // 1. Nothing saved.
  if (savedFiles.length === 0) {
    if (refused.length === 1) {
      return {
        tone: "error",
        sticky: false,
        message: endSentence(
          `Could not save ${refused[0].requestedName}: ${refused[0].reason}`,
        ),
      }
    }
    return {
      tone: "error",
      sticky: false,
      message: endSentence(
        `Could not save any of the ${refused.length} dropped files. ${reasonList(refused)}`,
      ),
    }
  }

  // 2. Something saved whose path never went out: we do not hold input, or the
  // socket was closed. This is the rung where the user finds the file by hand.
  if (notSent.length > 0) {
    const groups = strandedByReason(notSent)
    const alsoRefused =
      refused.length > 0 ? ` ${reasonList(refused)} was not saved at all.` : ""
    // One reason for all of them is the only case where a single "not sent:
    // <why>" clause is true of every stranded file.
    const head =
      groups.length === 1
        ? `Saved${toPhrase(savedFiles, ctx)}, but the path was ${notDelivered(ctx)}: ${endSentence(groups[0].reason)} ` +
          `The file is at ${strandedList(groups[0].files)}.`
        : `Saved${toPhrase(savedFiles, ctx)}, but ${notSent.length} paths were ${notDelivered(ctx)}: ` +
          `${groups
            .map((g) => `${strandedList(g.files)} because ${asClause(g.reason)}`)
            .join("; ")}.`
    return {
      tone: "warning",
      // Sticky: nothing else on screen names the path of a file that is on disk
      // and was never given to the agent.
      sticky: true,
      message:
        head +
        alsoRefused +
        renameNote(savedFiles, ctx) +
        folderBreakdown(savedFiles, ctx),
    }
  }

  // 3. Everything that saved was sent, but something was refused outright.
  if (refused.length > 0) {
    const total = outcomes.length
    return {
      tone: "warning",
      // Not sticky: the originals are still wherever they were dragged from.
      sticky: false,
      message:
        `Saved ${savedFiles.length} of ${total} files${toPhrase(savedFiles, ctx)} and ${deliveredMany(ctx)}. ` +
        `Refused: ${endSentence(reasonList(refused))}` +
        renameNote(savedFiles, ctx) +
        folderBreakdown(savedFiles, ctx),
    }
  }

  // 4. Everything worked.
  if (savedFiles.length === 1) {
    const one = savedFiles[0]
    const where = toPhrase(savedFiles, ctx)
    const named =
      one.requestedName === one.savedName
        ? `Saved ${one.savedName}${where} and ${deliveredOne(ctx)}.`
        : `Saved ${one.requestedName}${where} as ${one.savedName}, so nothing was overwritten, and ${deliveredOne(ctx)}.`
    return { tone: "success", sticky: false, message: named }
  }
  return {
    tone: "success",
    sticky: false,
    message:
      `Saved ${savedFiles.length} files${toPhrase(savedFiles, ctx)} and ${deliveredMany(ctx)}.` +
      renameNote(savedFiles, ctx) +
      folderBreakdown(savedFiles, ctx),
  }
}

/// The toast id one drop lives on: its per-file spinners and its final report,
/// so the final replaces that spinner in place. Minted per drop, because two
/// overlapping drops sharing an id paint over each other's report, and shared by
/// both drop surfaces, so neither can mint an id the other is using.
let fileDropSeq = 0
export function nextFileDropToastId(): string {
  fileDropSeq += 1
  return `file-drop-${fileDropSeq}`
}
