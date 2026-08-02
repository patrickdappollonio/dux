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

/// What became of one dropped file. Exactly three endings, and the toast is
/// chosen from an ORDERED list of these in the order the files were dropped,
/// which is also the order their paths are pasted.
export type DropOutcome =
  /// Saved, and the path was handed to an open socket we own.
  | { kind: "pasted"; requestedName: string; savedName: string; path: string }
  /// Saved, but the path was NOT sent: we do not hold input, or the socket was
  /// closed. The user has to be able to reach the file by hand, so this carries
  /// the full path.
  | {
      kind: "saved-not-sent"
      requestedName: string
      savedName: string
      path: string
      reason: string
    }
  /// Never saved. The reason is the server's own words, not a generic one.
  | { kind: "refused"; requestedName: string; reason: string }

/// Where the files went, in the terms the message should use.
export type DropContext = {
  /// An agent's destination is described as its worktree root, which reads
  /// better than a long path; a terminal's is the real directory.
  kind: "agent" | "terminal"
  /// The directory, already shortened with `~` by the server (which is the
  /// machine whose home directory it is).
  folderLabel: string
}

export type DropToast = {
  tone: "success" | "warning" | "error"
  message: string
}

/// Quote `path` as exactly ONE shell token.
///
/// The whole path is quoted, not just the filename. An earlier design claimed
/// that validating the filename removed the need to quote; that was wrong,
/// because the filename is only the last part. The DIRECTORY routinely contains
/// spaces, since a worktree path is built from the project's name.
///
/// Single quotes, because inside them every character is literal; the one
/// escape needed is for a single quote itself, which closes, escapes and
/// reopens. Codex's source unquotes both quote styles, and GNOME Terminal and
/// Konsole both quote dropped paths this way.
export function quoteShellToken(path: string): string {
  return `'${path.replaceAll("'", "'\\''")}'`
}

/// What to paste for one saved file: the quoted path, one trailing space, and
/// NO newline.
///
/// One file per paste. In these tools a newline SUBMITS, so a file arriving with
/// an automatic submit would fire a half-written prompt. Several files means
/// several pastes in sequence, because Codex only treats a pasted path as an
/// attachment when it parses as exactly one token, so two paths in one paste
/// become plain text.
export function pastePayload(path: string): string {
  return `${quoteShellToken(path)} `
}

function folderPhrase(ctx: DropContext): string {
  return ctx.kind === "agent" ? "the agent's worktree root" : ctx.folderLabel
}

function reasonList(items: { requestedName: string; reason: string }[]): string {
  if (items.length > MAX_NAMED_FILES) {
    return `${items.length} files were refused; the first was ${items[0].requestedName} (${items[0].reason}).`
  }
  return items.map((r) => `${r.requestedName} (${r.reason})`).join(", ")
}

function renameNote(
  saved: { requestedName: string; savedName: string }[],
  ctx: DropContext,
): string {
  const renamed = saved.filter((s) => s.requestedName !== s.savedName)
  if (renamed.length === 0) return ""
  // Named, never counted: a count says something changed without saying what
  // the file is now called, which is the whole reason for mentioning it.
  if (renamed.length > MAX_NAMED_FILES) {
    return ` ${renamed.length} already existed and were saved under new names, which are listed in ${folderPhrase(ctx)}.`
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
export function dropToastFor(
  outcomes: DropOutcome[],
  ctx: DropContext,
): DropToast {
  const pastedFiles = outcomes.filter((o) => o.kind === "pasted")
  const notSent = outcomes.filter((o) => o.kind === "saved-not-sent")
  const refused = outcomes.filter((o) => o.kind === "refused")
  const savedFiles = [...pastedFiles, ...notSent]
  const where = folderPhrase(ctx)

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
  // was closed. (A path handed to an open socket is claimed no more strongly
  // than any keystroke, because nothing acknowledges it.)
  if (notSent.length > 0) {
    const stranded = notSent
      .map((n) => `${n.savedName} (${n.path})`)
      .slice(0, MAX_NAMED_FILES)
      .join(", ")
    const overflow =
      notSent.length > MAX_NAMED_FILES
        ? ` and ${notSent.length - MAX_NAMED_FILES} more in ${where}`
        : ""
    const why = notSent[0].reason
    const alsoRefused =
      refused.length > 0 ? ` ${reasonList(refused)} was not saved at all.` : ""
    return {
      tone: "warning",
      message:
        `Saved to ${where}, but the path was not sent: ${why}. ` +
        `The file is at ${stranded}${overflow}.${alsoRefused}`,
    }
  }

  // 3. Everything that saved was pasted, but something was refused outright.
  if (refused.length > 0) {
    const total = outcomes.length
    return {
      tone: "warning",
      message:
        `Saved ${savedFiles.length} of ${total} files to ${where} and pasted their paths. ` +
        `Refused: ${reasonList(refused)}.` +
        renameNote(savedFiles, ctx),
    }
  }

  // 4. Everything worked.
  if (savedFiles.length === 1) {
    const one = savedFiles[0]
    const named =
      one.requestedName === one.savedName
        ? `Saved ${one.savedName} to ${where} and pasted its path.`
        : `Saved ${one.requestedName} to ${where} as ${one.savedName}, so nothing was overwritten, and pasted its path.`
    return { tone: "success", message: named }
  }
  return {
    tone: "success",
    message:
      `Saved ${savedFiles.length} files to ${where} and pasted their paths.` +
      renameNote(savedFiles, ctx),
  }
}
