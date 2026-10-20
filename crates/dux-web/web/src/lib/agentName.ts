// Agent-name rules, ported from `dux_core::git` so the web dialogs accept
// exactly the names the TUI does: any name git accepts as a branch. The TUI
// refuses an offending keystroke (`agent_name_char_map`); a React input hands
// over the whole next value, so `sanitizeAgentName` maps it character by
// character to the same result: a space becomes a dash, and a character no
// agent name may hold anywhere is dropped. Everything else stays, and
// `isValidAgentName` judges the whole name at submit.

// What git refuses anywhere in a ref name (ASCII control characters and DEL,
// space, and ~ ^ : ? * [ \), and the bidirectional controls `lib/bidi.ts`
// strips, which git takes but which would make a name draw as another.
// eslint-disable-next-line no-control-regex
const NEVER_IN_AN_AGENT_NAME = /[\u0000-\u001f\u007f ~^:?*[\\\u202A-\u202E\u2066-\u2069\u200E\u200F\u061C]/

/** Map a full input value through the per-character rule above. */
export function sanitizeAgentName(next: string): string {
  let out = ""
  for (const raw of next) {
    const ch = raw === " " ? "-" : raw
    if (NEVER_IN_AN_AGENT_NAME.test(ch)) continue
    out += ch
  }
  return out
}

/**
 * Exact port of `dux_core::git::is_valid_agent_name`, the rules of
 * `git check-ref-format --branch`; both are pinned by the shared
 * `agent_name_cross_language.json` vectors. Used to gate the submit button.
 */
export function isValidAgentName(name: string): boolean {
  if (name === "" || name.startsWith("-") || name === "HEAD" || name === "@") {
    return false
  }
  if (NEVER_IN_AN_AGENT_NAME.test(name)) return false
  if (name.includes("..") || name.includes("@{") || name.endsWith(".")) {
    return false
  }
  return name
    .split("/")
    .every(
      (part) =>
        part !== "" && !part.startsWith(".") && !part.endsWith(".lock"),
    )
}

/**
 * The generated-name bookkeeping both name dialogs share. Typing a name keeps
 * the remembered generated one only while the text still equals it, so a later
 * uncheck can tell a name the user wrote from one dux filled in.
 */
export function generatedNameAfterEdit(
  draft: string,
  generated: string | null,
): string | null {
  return draft === generated ? generated : null
}

/**
 * What the name field holds after the pet-name box is unchecked: cleared when
 * it still holds the generated name, untouched when the user has edited it.
 */
export function draftAfterRandomizeOff(
  draft: string,
  generated: string | null,
): string {
  return draft === generated ? "" : draft
}
