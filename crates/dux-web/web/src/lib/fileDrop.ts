// Dropping a file onto a terminal or agent pane: what gets pasted, and what the
// user is told afterwards.
//
// Both halves are pure and live here rather than in the pane, so they are
// testable without mounting xterm and so the ordering rules cannot drift into a
// component closure where nobody can see them.
//
// The premise, settled and not to be re-litigated: sending a file's BYTES to the
// terminal cannot work. No agent CLI reads a file from its input stream; they
// take a path, or they read the clipboard of the machine THEY run on, which for
// a browser user is the wrong computer. Every terminal emulator whose source was
// read inserts the path on a drop. So dux saves the file and pastes its path.

/// How many file names a single toast will spell out before it gives up and
/// points at the folder instead. Past this the message is longer than anyone
/// reads and the folder listing is the better answer.
export const MAX_NAMED_FILES = 5

/// What one SAVED file has in common whichever way it ended.
///
/// The folder belongs HERE rather than to the drop as a whole. A terminal's
/// directory changes the moment someone types `cd`, and the uploads are
/// sequential, so two files dropped together can genuinely land in two different
/// folders. Keeping one folder for the whole drop meant the last upload's folder
/// was reported for every file in it.
export type SavedFile = {
  requestedName: string
  savedName: string
  /// The absolute path, which is what the user needs when the path was not sent.
  path: string
  /// The folder, already shortened with `~` by the server (which is the machine
  /// whose home directory it is).
  folderLabel: string
}

/// What became of one dropped file. Exactly three endings, and the toast is
/// chosen from an ORDERED list of these in the order the files were dropped,
/// which is also the order their paths are sent.
export type DropOutcome =
  /// Saved, and the path was written to an open socket we own.
  ///
  /// Called SENT rather than "pasted" deliberately. Nothing acknowledges a
  /// write to the PTY socket, and a take-over between the courtesy check and the
  /// frame reaching the server makes the server drop it silently, so what dux
  /// knows is that it sent the path, never that the path arrived.
  | ({ kind: "sent" } & SavedFile)
  /// Saved, but the path was NOT sent: we do not hold input, or the socket was
  /// closed. The user has to be able to reach the file by hand, so this carries
  /// the full path.
  | ({ kind: "saved-not-sent"; reason: string } & SavedFile)
  /// Never saved. The reason is the server's own words, not a generic one.
  | { kind: "refused"; requestedName: string; reason: string }

/// How the destination should be DESCRIBED, which is the one thing that is a
/// property of the whole drop rather than of a file.
export type DropContext = {
  /// An agent's destination is described as its worktree root, which reads
  /// better than a long path and is the same for every file, because every tab
  /// of one agent shares one worktree. A terminal's is the real directory, which
  /// each saved file carries for itself.
  kind: "agent" | "terminal"
}

export type DropToast = {
  tone: "success" | "warning" | "error"
  message: string
}

/// The form a dropped file's path takes when it is written into the prompt.
///
/// Mirrors `dux_core::config::WebDragDropPaste`, and the names are the exact
/// strings the server publishes in `provider_web_dragdrop_paste`, so nothing has
/// to translate between the two.
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

/// Which form to use for the provider running in the pane being dropped on.
///
/// `forms` is the server's `bootstrap.provider_web_dragdrop_paste` map, keyed by
/// provider name. `bare` is the answer for everything the map does not cover: a
/// provider the user added themselves, a server too old to send the map, a pane
/// with no provider at all (a plain terminal), and, defensively, a form name this
/// client does not know. The server already normalizes and already warned about a
/// misspelling once at config load, so the last case should not arise; a client
/// that trusted the string blindly would still be one config typo away from
/// pasting the literal word into somebody's prompt.
export function dragDropPasteFormFor(
  forms: Record<string, string> | undefined,
  provider: string | undefined,
): DragDropPasteForm {
  const named = provider === undefined ? undefined : forms?.[provider]
  return DRAG_DROP_PASTE_FORMS.find((f) => f === named) ?? "bare"
}

/// The characters `backslashEscaped` protects: whitespace, the quoting and
/// expansion characters, the shell's own operators, and the glob characters.
///
/// Deliberately ASCII-only and deliberately not "everything that is not a letter".
/// A backslash before an ordinary character is a no-op in POSIX lexing, so
/// over-escaping is harmless to the LEXER, but it is not harmless to a reader, and
/// escaping every CJK codepoint in a path would make the prompt unreadable for the
/// exact users most likely to have one.
const SHELL_SIGNIFICANT = /[\s"#$&'()*;<>?[\\\]`{|}~]/g

/// Wrap in single quotes, closing and reopening around each embedded apostrophe.
///
/// Inside POSIX single quotes NOTHING is special, not `$`, not a backtick, not a
/// backslash, so the apostrophe is the only character that needs handling and the
/// only way to include one is to leave the quotes, escape it, and go back in.
/// Escaping anything else here would be quoting for a shell that does not exist.
function singleQuoted(path: string): string {
  return `'${path.replaceAll("'", `'\\''`)}'`
}

/// Wrap in double quotes, escaping the two characters that would otherwise end or
/// alter the quoting.
///
/// A shell EVALUATOR also expands `$` and a backtick inside double quotes, but the
/// thing on the receiving end is a LEXER deciding how many words it is looking at,
/// and a lexer leaves both alone. Escaping them would change the bytes the CLI
/// finally sees for no gain.
function doubleQuoted(path: string): string {
  return `"${path.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`
}

/// No quotes; escape each shell-significant character on its own.
function backslashEscaped(path: string): string {
  return path.replace(SHELL_SIGNIFICANT, (c) => `\\${c}`)
}

/// What to paste for one saved file: the path in the form this provider needs,
/// one trailing space, and NO newline.
///
/// Pure, and takes the form rather than reading it, so every case is testable
/// without mounting a terminal.
///
/// WHY THIS IS PER-PROVIDER. The receiving end is an agent CLI, not a shell, and
/// the CLIs do not agree on how they read a pasted path. `dux_core::config::
/// WebDragDropPaste` carries the measured table of what each one does and which
/// form it therefore needs, along with the two combinations known to FAIL. In
/// short, and measured rather than assumed:
///
///   - Claude Code and OpenCode take the WHOLE pasted string and never split on
///     whitespace, so `bare` is right and a space is harmless. Single-quoting a
///     path with an APOSTROPHE actively breaks Claude Code, because POSIX writes
///     that apostrophe as a close-escape-reopen and Claude Code's own unescape
///     step collapses it into three apostrophes.
///   - Codex lexes the text with POSIX shell rules and accepts it only if it comes
///     out as exactly ONE token, so a bare path containing a space is silently
///     ignored and `single_quoted` is what it needs.
///
/// KNOWN LIMITATION, stated rather than worked around: a path containing a
/// BACKSLASH is mangled by Claude Code's unescape step in EVERY form, because the
/// unescape eats the backslash. That is a property of the receiving tool and dux
/// cannot fix it from this side.
///
/// One file per paste. In these tools a newline SUBMITS, so a file arriving with
/// an automatic submit would fire a half-written prompt. Several files means
/// several pastes in sequence, because these tools only treat a pasted path as an
/// attachment when the whole pasted string is that one path, so two paths in one
/// paste become plain text.
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

/// How to describe where the drop went, when ONE phrase can honestly cover it.
///
/// Empty when the terminal's files went to more than one folder: no single
/// phrase is true then, and claiming one is the bug this exists to prevent. The
/// caller reaches for `folderBreakdown` instead.
function folderPhrase(saved: SavedFile[], ctx: DropContext): string {
  if (ctx.kind === "agent") return "the agent's worktree root"
  const folders = foldersOf(saved)
  if (folders.length === 1) return folders[0]
  // No folder at all can only happen if the server sent an empty label; say
  // something true rather than "undefined".
  return folders.length === 0 ? "the terminal's folder" : ""
}

/// The per-folder listing used when one phrase cannot cover the drop.
///
/// Grouped rather than enumerated per file, so three files in two folders read
/// as two clauses instead of three.
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
  return ` A terminal moves, so they did not all land together: ${clauses.join(", ")}.`
}

/// `to <somewhere>` when one phrase covers the drop, and nothing when it does
/// not, because `folderBreakdown` then says it properly.
function toPhrase(saved: SavedFile[], ctx: DropContext): string {
  const where = folderPhrase(saved, ctx)
  return where === "" ? "" : ` to ${where}`
}

/// The stranded files that share a reason, grouped, in the order the reasons
/// were first hit (which is the order the files were dropped).
///
/// Grouped rather than one clause per file, for the same reason
/// `folderBreakdown` groups by folder: five files stranded by one reconnect
/// should read as one clause, not five. And grouped rather than reduced to the
/// FIRST reason, which is what discarded the later ones: the uploads are
/// sequential, so a reconnect stranding one file and a take-over stranding the
/// next is an ordinary drop, not a corner case.
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

/// Stranded files named with their full paths, because this is the rung where
/// the user has to go and find them by hand. Capped, with the remainder counted
/// rather than dropped silently.
function strandedList(files: SavedFile[]): string {
  const named = files
    .slice(0, MAX_NAMED_FILES)
    .map((f) => `${f.savedName} (${f.path})`)
    .join(", ")
  return files.length > MAX_NAMED_FILES
    ? `${named} and ${files.length - MAX_NAMED_FILES} more`
    : named
}

function reasonList(items: { requestedName: string; reason: string }[]): string {
  if (items.length > MAX_NAMED_FILES) {
    return `${items.length} files were refused; the first was ${items[0].requestedName} (${items[0].reason}).`
  }
  return items.map((r) => `${r.requestedName} (${r.reason})`).join(", ")
}

/// The renamed-file note, applied to EVERY saved file at EVERY rung.
///
/// It used to be dropped whenever the toast landed on a worse rung, which lost
/// the original-to-saved pair exactly when the user needed it most: a file that
/// was renamed AND whose path never went out is one they have to find by hand
/// under a name they were never told.
function renameNote(saved: SavedFile[], ctx: DropContext): string {
  const renamed = saved.filter((s) => s.requestedName !== s.savedName)
  if (renamed.length === 0) return ""
  // Named, never counted: a count says something changed without saying what
  // the file is now called, which is the whole reason for mentioning it.
  if (renamed.length > MAX_NAMED_FILES) {
    const where = folderPhrase(renamed, ctx)
    return ` ${renamed.length} already existed and were saved under new names, which are listed in ${where === "" ? "the folders above" : where}.`
  }
  const pairs = renamed
    .map((r) => `${r.requestedName} was saved as ${r.savedName}`)
    .join(", ")
  return ` ${pairs}, so nothing was overwritten.`
}

/// The ONE toast for a whole drop, chosen from the ordered per-file outcomes.
///
/// One toast rather than one per file, so a handful of files does not bury the
/// screen. The rung is the FIRST of these that applies, so a bad outcome can
/// never be reported as a good one:
///
///   1. nothing saved              -> error
///   2. anything saved but not sent -> warning (names those files and their
///                                     full paths, because the user now has to
///                                     reference them by hand)
///   3. anything refused            -> warning
///   4. otherwise                   -> success
///
/// Two things are said at EVERY rung that has a saved file, whichever one it
/// lands on: what a renamed file is now called, and which folder each file went
/// to when they did not all go to the same one.
export function dropToastFor(
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
        message: `Could not save ${refused[0].requestedName}: ${refused[0].reason}.`,
      }
    }
    return {
      tone: "error",
      message: `Could not save any of the ${refused.length} dropped files. ${reasonList(refused)}`,
    }
  }

  // 2. Something saved whose path never went out. Precise about what "sent"
  // means: we KNOW these did not go, because we do not hold input or the socket
  // was closed. (A path written to an open socket is claimed no more strongly
  // than any keystroke, because nothing acknowledges it.)
  //
  // The rename note and the folder breakdown belong here as much as anywhere:
  // this is the rung where the user has to go and find the file themselves, so
  // a name they were never told and a folder they were told wrongly are worse
  // here than on any other rung.
  if (notSent.length > 0) {
    const groups = strandedByReason(notSent)
    const alsoRefused =
      refused.length > 0 ? ` ${reasonList(refused)} was not saved at all.` : ""
    // One reason for all of them is the ordinary case and reads plainly. It is
    // ALSO the only case where a single "not sent: <why>" clause is true, which
    // is what the previous version said unconditionally, taking the first
    // stranded file's reason and applying it to every one of them.
    const head =
      groups.length === 1
        ? `Saved${toPhrase(savedFiles, ctx)}, but the path was not sent: ${groups[0].reason}. ` +
          `The file is at ${strandedList(groups[0].files)}.`
        : `Saved${toPhrase(savedFiles, ctx)}, but ${notSent.length} paths were not sent: ` +
          `${groups
            .map((g) => `${strandedList(g.files)} because ${g.reason}`)
            .join("; ")}.`
    return {
      tone: "warning",
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
      message:
        `Saved ${savedFiles.length} of ${total} files${toPhrase(savedFiles, ctx)} and sent their paths. ` +
        `Refused: ${reasonList(refused)}.` +
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
        ? `Saved ${one.savedName}${where} and sent its path.`
        : `Saved ${one.requestedName}${where} as ${one.savedName}, so nothing was overwritten, and sent its path.`
    return { tone: "success", message: named }
  }
  return {
    tone: "success",
    message:
      `Saved ${savedFiles.length} files${toPhrase(savedFiles, ctx)} and sent their paths.` +
      renameNote(savedFiles, ctx) +
      folderBreakdown(savedFiles, ctx),
  }
}
