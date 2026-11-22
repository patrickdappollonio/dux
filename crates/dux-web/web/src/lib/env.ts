// Shared helpers for editing environment variables as `KEY=VALUE` text. Used by
// the global-env and per-project settings dialogs so the parse/serialize rules
// stay identical.

// Render an env map as sorted `KEY=VALUE` lines for a textarea.
export function envToText(env: Record<string, string>): string {
  return Object.entries(env)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([k, v]) => `${k}=${v}`)
    .join("\n")
}

// One line of `KEY=VALUE` text: a variable, nothing (a blank line), or null for
// a line that is not a variable (a `#` comment, or no key before an `=`). The
// first `=` splits, so a value may itself contain `=`.
function parseLine(raw: string): [string, string] | "blank" | null {
  const line = raw.trim()
  if (line === "") return "blank"
  if (line.startsWith("#")) return null
  const eq = line.indexOf("=")
  if (eq <= 0) return null
  return [line.slice(0, eq).trim(), line.slice(eq + 1)]
}

// Parse `KEY=VALUE` text back to an object, skipping every line that is not a
// variable.
export function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {}
  for (const raw of text.split("\n")) {
    const parsed = parseLine(raw)
    if (Array.isArray(parsed)) env[parsed[0]] = parsed[1]
  }
  return env
}

// How many variables differ between two env maps: added, removed or given
// another value.
export function envChangeCount(
  before: Record<string, string>,
  after: Record<string, string>,
): number {
  const keys = new Set([...Object.keys(before), ...Object.keys(after)])
  let changed = 0
  for (const key of keys) {
    if (before[key] !== after[key]) changed += 1
  }
  return changed
}

/** What an environment field's text comes to against the table it edits. */
export interface EnvEdit {
  /** The table Save sends. */
  env: Record<string, string>
  /** The variables Save would add, remove or change. */
  changed: number
  /** The 1-based numbers of the lines Save leaves out. */
  leftOut: number[]
  /** What closing would lose: a change per variable and per line left out,
   * and at least one whenever the text differs from what the field opened
   * with, whatever it parses to. */
  edits: number
}

export function envEdit(
  saved: Record<string, string>,
  opened: string,
  text: string,
): EnvEdit {
  const env = parseEnv(text)
  const leftOut: number[] = []
  text.split("\n").forEach((raw, i) => {
    if (parseLine(raw) === null) leftOut.push(i + 1)
  })
  const changed = envChangeCount(saved, env)
  const edits = changed + leftOut.length
  return { env, changed, leftOut, edits: edits === 0 && text !== opened ? 1 : edits }
}

/** The sentence under an environment field that names the lines Save leaves
 * out, or null when it keeps every line. */
export function envLeftOutNote(leftOut: readonly number[]): string | null {
  if (leftOut.length === 0) return null
  if (leftOut.length === 1) return `Line ${leftOut[0]} is not KEY=VALUE, so Save leaves it out.`
  const head = leftOut.slice(0, -1).join(", ")
  return `Lines ${head} and ${leftOut[leftOut.length - 1]} are not KEY=VALUE, so Save leaves them out.`
}
