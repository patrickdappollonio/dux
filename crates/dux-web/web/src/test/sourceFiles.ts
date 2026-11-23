// Shared by the guards that read the web UI's own source: which files they read
// and the source with its comments taken out. Reading text, not the program.

// The app's type check reads this file and has no node types, so node's file
// system is named through a variable it does not resolve, and the two calls
// made of it are typed here. Only tests import this module.
interface FileSystem {
  readdirSync(dir: string, options: { withFileTypes: true }): { name: string; isDirectory(): boolean }[]
  readdirSync(dir: string): string[]
  statSync(path: string): { isDirectory(): boolean }
}
const NODE_FS = "node:fs"
const { readdirSync, statSync } = (await import(/* @vite-ignore */ NODE_FS)) as FileSystem

const join = (dir: string, name: string) => `${dir.replace(/\/+$/, "")}/${name}`

/** Every file under `dir`, at any depth. */
export function filesUnder(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? filesUnder(path) : [path]
  })
}

/**
 * The `.ts` and `.tsx` files under `dir` that ship: tests and declaration files
 * left out, and `node_modules` and any folder named in `skipFolders` not entered.
 */
export function sourceFilesUnder(dir: string, skipFolders: readonly string[] = []): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) {
      return e.name === "node_modules" || skipFolders.includes(e.name) ? [] : sourceFilesUnder(p, skipFolders)
    }
    return /\.tsx?$/.test(e.name) && !/\.test\.|\.d\.ts$/.test(e.name) ? [p] : []
  })
}

/**
 * The source with every comment blanked to spaces, so each line and column
 * still points where it did. A comment start inside a string is not a
 * comment; a quote that does not close on its line (an apostrophe in JSX text)
 * ends there, so it cannot swallow the code after it. A template literal is
 * read as one string, `${}` and all, and a regular expression is not known
 * for what it is, so a quote or `//` inside one can hide the rest of its line.
 */
export function withoutComments(source: string): string {
  let out = ""
  let i = 0
  let quote: string | null = null
  while (i < source.length) {
    const c = source[i]
    const next = source[i + 1]
    if (quote !== null) {
      out += c
      if (c === "\\" && next !== undefined && next !== "\n") {
        out += next
        i += 2
        continue
      }
      if (c === quote || (c === "\n" && quote !== "`")) quote = null
      i++
    } else if (c === "/" && next === "/") {
      while (i < source.length && source[i] !== "\n") {
        out += " "
        i++
      }
    } else if (c === "/" && next === "*") {
      const end = source.indexOf("*/", i + 2)
      const stop = end === -1 ? source.length : end + 2
      out += source.slice(i, stop).replace(/[^\n]/g, " ")
      i = stop
    } else {
      if (c === '"' || c === "'" || c === "`") quote = c
      out += c
      i++
    }
  }
  return out
}

/**
 * The source with each comment turned into one space, read more roughly: every
 * string, a template literal too, ends on the line it starts on, so a template
 * that spans lines is read as code from its second line on. Lines and columns
 * are not kept.
 */
export function withoutCommentsLineStrings(source: string): string {
  return source.replace(
    /(["'`])(?:\\.|(?!\1)[^\\\n])*\1|\/\*[\s\S]*?\*\/|\/\/[^\n]*/g,
    (m) => (m.startsWith("/") ? " " : m),
  )
}
