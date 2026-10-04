// The password strength meter and the rules Preferences checks before it sends
// a new password.
//
// The estimate is zxcvbn's (the zxcvbn-ts port, with its common and English
// dictionaries), on the same 0-4 scale the server's Rust port scores on, so the
// meter and the server's refusal speak the same numbers. The server is still
// the one that decides: the two ports' dictionaries are close but not
// identical, so a password the meter calls Good can come back refused, and the
// refusal is shown as the server words it.
//
// The dictionaries are several megabytes, so they load on first use, in their
// own chunk, rather than with the app.

import type { ZxcvbnFactory } from "@zxcvbn-ts/core"

export const STRENGTH_LABELS = ["Weak", "Fair", "Good", "Strong", "Excellent"] as const

export type StrengthScore = 0 | 1 | 2 | 3 | 4

export interface Strength {
  score: StrengthScore
  label: string
  /** zxcvbn's warning, else its first suggestion, else null. */
  hint: string | null
}

/// The documented defaults of `minimum_password_length` and
/// `minimum_password_score` in `[server.auth]`, used when the status does not
/// report the configured values.
export const DEFAULT_MINIMUM_PASSWORD_LENGTH = 12
export const DEFAULT_MINIMUM_PASSWORD_SCORE = 2

export interface PasswordMinimums {
  length: number
  score: number
}

export function passwordMinimums(status: {
  minimum_password_length: number | null
  minimum_password_score: number | null
}): PasswordMinimums {
  return {
    length: status.minimum_password_length ?? DEFAULT_MINIMUM_PASSWORD_LENGTH,
    score: status.minimum_password_score ?? DEFAULT_MINIMUM_PASSWORD_SCORE,
  }
}

let factory: Promise<ZxcvbnFactory> | null = null

function loadFactory(): Promise<ZxcvbnFactory> {
  if (factory) return factory
  factory = Promise.all([
    import("@zxcvbn-ts/core"),
    import("@zxcvbn-ts/language-common"),
    import("@zxcvbn-ts/language-en"),
  ]).then(
    ([core, common, en]) =>
      new core.ZxcvbnFactory({
        dictionary: { ...common.dictionary, ...en.dictionary },
        graphs: common.adjacencyGraphs,
        translations: en.translations,
      }),
  )
  // A failed load (a dropped chunk) is retried on the next keystroke rather
  // than remembered.
  factory.catch(() => {
    factory = null
  })
  return factory
}

export async function estimateStrength(password: string): Promise<Strength> {
  const zxcvbn = await loadFactory()
  const result = zxcvbn.check(password)
  const score = Math.max(0, Math.min(4, result.score)) as StrengthScore
  const hint = result.feedback.warning || result.feedback.suggestions[0] || null
  return { score, label: STRENGTH_LABELS[score], hint }
}

export interface PasswordDraft {
  current: string
  next: string
  confirm: string
}

export type PasswordWrite =
  | { kind: "none" }
  | { kind: "invalid"; message: string }
  | { kind: "write"; current?: string; next: string }

/// What Save does with the password fields: nothing (left empty), refuse with a
/// sentence, or send. `strength` is the meter's reading of `next`, null while it
/// is still being computed.
export function passwordWrite(
  draft: PasswordDraft,
  ctx: { passwordSet: boolean; mins: PasswordMinimums; strength: Strength | null },
): PasswordWrite {
  if (draft.current === "" && draft.next === "" && draft.confirm === "") {
    return { kind: "none" }
  }
  if (ctx.passwordSet && draft.current === "") {
    return { kind: "invalid", message: "Type your current password to change it." }
  }
  // Characters, not UTF-16 units or bytes, matching what the user counts.
  if ([...draft.next].length < ctx.mins.length) {
    return { kind: "invalid", message: `Use at least ${ctx.mins.length} characters.` }
  }
  if (ctx.strength === null) {
    return {
      kind: "invalid",
      message: "Still checking how strong that password is. Try Save again in a moment.",
    }
  }
  if (ctx.strength.score < ctx.mins.score) {
    const wanted = STRENGTH_LABELS[Math.max(0, Math.min(4, ctx.mins.score))]
    const hint = ctx.strength.hint ? ` ${ctx.strength.hint}` : ""
    return {
      kind: "invalid",
      message: `That password is too easy to guess (${ctx.strength.label}; dux asks for at least ${wanted}).${hint}`,
    }
  }
  if (draft.confirm !== draft.next) {
    return { kind: "invalid", message: "The two new passwords do not match." }
  }
  return ctx.passwordSet
    ? { kind: "write", current: draft.current, next: draft.next }
    : { kind: "write", next: draft.next }
}
