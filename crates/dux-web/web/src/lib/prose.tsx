import type { ReactNode } from "react"

import { InlineCode } from "@/components/ui/inline-code"
import { stripBidiControls } from "@/lib/bidi"

// A sentence that names things, kept as structure rather than as a finished
// string, so the web can draw each name as a chip without hunting for it in the
// rendered words.
//
// Some sentences are also the terminal UI's, byte for byte, and there a name may
// be wrapped in straight quotes. `quotedChip` records that, so the plain-text
// form (`proseText`) reproduces the terminal's string exactly while the web form
// (`renderProse`) drops the quotes in favour of the chip. One wording, two
// spellings, and no way for them to disagree about the words.

export interface ProseName {
  name: string
  // The plain-text spelling wraps this name in straight double quotes.
  quoted: boolean
}

export type ProseSegment = string | ProseName

export type Prose = readonly ProseSegment[]

// A name is where text dux did not write (a branch, a pull request's head, a
// folder) enters a sentence, so the two builders drop its bidi controls: a chip
// is drawn as one unit inside dux's own words, and an override inside it would
// reorder them. The same rule as `dux_core::prose`'s name builders.

/** A name that the plain-text spelling leaves bare. */
export function chip(name: string): ProseName {
  return { name: stripBidiControls(name), quoted: false }
}

/** A name that the plain-text spelling wraps in straight double quotes. */
export function quotedChip(name: string): ProseName {
  return { name: stripBidiControls(name), quoted: true }
}

/** The plain-text spelling: what the terminal UI says, word for word. */
export function proseText(prose: Prose): string {
  return prose
    .map((segment) =>
      typeof segment === "string"
        ? segment
        : segment.quoted
          ? `"${segment.name}"`
          : segment.name,
    )
    .join("")
}

// Append one segment, merging adjacent words so two builders that say the same
// thing produce equal values however they split the constant parts.
function pushSegment(out: ProseSegment[], segment: ProseSegment): void {
  if (typeof segment === "string") {
    if (segment === "") return
    const last = out.length - 1
    if (last >= 0 && typeof out[last] === "string") {
      out[last] = (out[last] as string) + segment
      return
    }
  }
  out.push(segment)
}

/** A value a `prose` template can interpolate. */
export type ProsePart = string | number | ProseName | Prose

/**
 * Build a sentence as a template: a `chip(...)` or `quotedChip(...)` value is a
 * name, a whole `Prose` is spliced in, and any other value is words. The way a
 * sentence the browser writes itself (a toast it raises, say) names things
 * without anyone hunting for the names in the finished string.
 */
export function prose(
  strings: TemplateStringsArray,
  ...values: ProsePart[]
): Prose {
  const out: ProseSegment[] = []
  strings.forEach((text, index) => {
    pushSegment(out, text)
    if (index >= values.length) return
    const value = values[index]
    if (Array.isArray(value)) {
      for (const segment of value as Prose) pushSegment(out, segment)
    } else if (typeof value === "object") {
      out.push(value as ProseName)
    } else {
      pushSegment(out, String(value))
    }
  })
  return out
}

/** Join sentences with a separator, the way `Array.join` joins strings. */
export function joinProse(parts: readonly Prose[], separator: string): Prose {
  const out: ProseSegment[] = []
  parts.forEach((part, index) => {
    if (index > 0) pushSegment(out, separator)
    for (const segment of part) pushSegment(out, segment)
  })
  return out
}

/**
 * End a sentence with exactly one terminator: trailing space dropped, and a
 * full stop added unless the plain spelling already ends in `.`, `!` or `?`.
 */
export function endProse(sentence: Prose): Prose {
  const out = [...sentence]
  const last = out.length - 1
  if (last >= 0 && typeof out[last] === "string") {
    const trimmed = (out[last] as string).trimEnd()
    if (trimmed === "") out.pop()
    else out[last] = trimmed
  }
  if (out.length === 0) return out
  return /[.!?]$/.test(proseText(out)) ? out : joinProse([out, ["."]], "")
}

function isProseSegment(value: unknown): value is ProseSegment {
  if (typeof value === "string") return true
  if (typeof value !== "object" || value === null) return false
  const candidate = value as { name?: unknown; quoted?: unknown }
  return typeof candidate.name === "string" && typeof candidate.quoted === "boolean"
}

/**
 * Read an engine status off the wire: its parts when the server sent them and
 * they spell the message exactly, the plain message otherwise.
 *
 * The server builds both from the same parts (`dux_core::status_text`), so a
 * disagreement means something upstream is wrong, and the plain message is the
 * one the terminal UI printed. Nothing here ever looks for a name inside the
 * words: a sentence without parts stays text.
 *
 * Bidi controls are dropped from both halves before they are compared: the
 * server already strips them from every name it builds, and an older server
 * that did not must not get to reorder a toast either.
 */
export function wireProse(message: string, segments: unknown): string | Prose {
  const plain = stripBidiControls(message)
  if (!Array.isArray(segments) || !segments.every(isProseSegment)) return plain
  const stripped: Prose = segments.map((segment) =>
    typeof segment === "string"
      ? stripBidiControls(segment)
      : { name: stripBidiControls(segment.name), quoted: segment.quoted },
  )
  if (proseText(stripped) === plain) return stripped
  // The fallback hides a producer upstream that is wrong, so say it where a
  // developer can find it.
  console.warn("[dux] status parts do not spell their message; showing the plain text", {
    message,
    segments,
  })
  return plain
}

/** The web spelling: every name drawn through the shared inline code chip. */
export function renderProse(prose: Prose): ReactNode[] {
  return prose.map((segment, index) =>
    typeof segment === "string" ? (
      segment
    ) : (
      <InlineCode key={index}>{segment.name}</InlineCode>
    ),
  )
}

// The three bodies of the dialog that offers a pull request's repository. The
// terminal UI builds the same segments (`dux_core::prose`), and the shared
// fixture pins the words.

/** No project is a checkout of the repository, and every project was checked. */
export function cloneOfferProse(repository: string, number: string | number): Prose {
  return prose`${chip(repository)} isn't a dux project yet. dux will clone it, add it as a project, and create the agent from pull request ${chip(`#${number}`)}.`
}

/** The same offer when some projects could not be checked. */
export function cloneOfferIncompleteProse(
  repository: string,
  number: string | number,
  summary: string,
): Prose {
  return prose`dux could not find a checked project for ${chip(repository)}. dux could not check every project (${summary}), so one of them may already be a checkout of it. dux will clone it, add it as a project, and create the agent from pull request ${chip(`#${number}`)}.`
}

/** The folder already holds a checkout of the repository. */
export function addExistingOfferProse(
  folder: string,
  repository: string,
  number: string | number,
): Prose {
  return prose`${chip(folder)} is already a checkout of ${chip(repository)}. dux will add it as a project and create the agent from pull request ${chip(`#${number}`)}.`
}
