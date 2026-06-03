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

// Parse `KEY=VALUE` text back to an object. Blank lines and `#` comments are
// skipped; the first `=` splits key and value so values may themselves contain
// `=`.
export function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {}
  for (const raw of text.split("\n")) {
    const line = raw.trim()
    if (line === "" || line.startsWith("#")) continue
    const eq = line.indexOf("=")
    if (eq <= 0) continue // skip lines with no key
    const key = line.slice(0, eq).trim()
    const value = line.slice(eq + 1) // keep value as-is (may contain '=')
    if (key) env[key] = value
  }
  return env
}
