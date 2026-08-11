// The header metadata strip is ONE ROW OF CHIPS, each a glyph followed by its
// value, and the decision of WHICH chips exist and what each one says lives here
// rather than in the components, so it can be unit-tested without mounting React
// and so the agent / session-terminal / project-terminal / standalone-terminal
// variants cannot drift apart.
//
// The shape this replaced was four labelled pairs (`agent: X | provider: Y |
// project: Z | branch: W`), then briefly a bold subject plus a muted caption.
// Two measured problems drove the first redesign and both still apply: the
// labels were 34 of about 74 characters, so nearly half the bar spelled out what
// the values already made obvious, and an agent named after its branch (the
// common case, `server-mode` on `server-mode`) printed that one word twice.
//
// So: the LABEL becomes a glyph, the glyph is also the separator (no hairline
// rules between fields, a wider gap does that work), and the word the glyph
// stands for is recovered on hover. That hover is not decoration: it is the only
// thing that makes a glyph learnable, and it is the reason this shape is
// acceptable at all. Every chip therefore carries a `label`, and a chip with no
// label is a bug rather than a style.

// Joins the clauses of a tooltip, and the phone header's one caption line. A
// middot rather than a hairline divider because it is one small run of text.
export const CAPTION_SEPARATOR = " · "

// The chips, in the order they render. `terminal` is one kind rather than two
// because a terminal glyph means the same thing in both places it appears: on an
// agent it counts that agent's terminals, and on a focused terminal it names the
// terminal you are looking at.
export type HeaderChipKind =
  | "project"
  | "agent"
  | "branch"
  | "terminal"
  | "assistant"
  | "directory"

export interface HeaderChip {
  kind: HeaderChipKind
  // The word the glyph stands for, shown on hover. Never empty.
  label: string
  // The chip's text.
  value: string
  // An extra tooltip clause after the label (and after the value when the value
  // is cut off): the assistant's "change it in the agent menu", the branch's
  // drift note, a terminal's sibling count.
  hint?: string
  // Exactly one chip per header is primary: the thing you navigate by. It is the
  // LAST to give way when the row runs out of room; every other chip yields
  // first, all the way to nothing.
  primary?: boolean
}

// The assistant chip's hover clause. The provider is not editable from the
// header, so the tooltip says where it IS editable rather than leaving a user
// hunting for it.
export const ASSISTANT_HINT = "change it in the agent menu"

// Drop empties and join. Callers pass optional parts straight through (a project
// that could not be resolved, a drift clause that does not apply) rather than
// filtering at each site.
export function captionText(
  parts: readonly (string | null | undefined)[],
): string {
  return parts.filter((p): p is string => !!p).join(CAPTION_SEPARATOR)
}

// What a chip says on hover. The label ALWAYS, because that is the whole deal
// with a glyph. The VALUE only when it is actually cut off on screen: a tooltip
// that repeats text the user can already read is noise, and whether the text is
// cut off is measurable at render (scroll width against client width), so the
// caller passes the answer in rather than guessing.
export function headerChipTooltip(chip: HeaderChip, truncated: boolean): string {
  return captionText([chip.label, truncated ? chip.value : null, chip.hint])
}

// The sibling-count clause, pluralized. Omitted (null) at zero so callers can
// pass the count unconditionally.
export function terminalCountCaption(count: number): string | null {
  if (count <= 0) return null
  return count === 1 ? "1 terminal" : `${count} terminals`
}

export interface AgentChipsInput {
  // The agent's display name (its title, falling back to its branch).
  name: string
  provider: string
  projectName?: string | null
  branchName: string
  // The immutable branch the agent was created on. Absent on an older server.
  initialBranch?: string | null
  // How many terminals this agent owns. Zero renders no terminal chip.
  terminalCount?: number
  // Set when a TERMINAL is the thing on screen and this agent merely owns it.
  // The agent then stops being the primary chip and the terminal takes over.
  primary?: "agent" | "none"
}

// True when the branch has moved off the one the agent was created on. Guarded
// on `initialBranch` being present, so an older server that omits the field
// never renders "originally undefined".
function branchDrifted(input: AgentChipsInput): boolean {
  return !!input.initialBranch && input.initialBranch !== input.branchName
}

// The branch chip, or null.
//
// It is omitted in the ordinary case, where the branch merely repeats the agent
// name the header is already showing (an untitled agent takes its name FROM its
// branch, so that case is the common one and printing it twice was the original
// complaint). It appears when the branch differs from that name.
//
// It ALSO appears when the branch has DRIFTED off the branch the agent was
// created on, even if it currently matches the agent name. That second condition
// is not in the mock, which never draws a drifted agent; without it the drift
// note has no chip to live on and the fact would be silently dropped, which is
// worse than one extra chip in a rare case.
export function branchChip(input: AgentChipsInput): HeaderChip | null {
  const drifted = branchDrifted(input)
  if (input.branchName === input.name && !drifted) return null
  return {
    kind: "branch",
    label: "Branch",
    value: input.branchName,
    hint: drifted ? `originally ${input.initialBranch}` : undefined,
  }
}

// The chips describing an AGENT, in order: project, agent, branch, terminals,
// assistant. Project leads because it is the coarsest fact and answers "which
// codebase am I in"; the agent name follows and is what you navigate by;
// branch and terminals appear only when they have something to say, which is
// what keeps the row short in the ordinary case; the assistant is last because
// it is the one value with a small fixed set and the one you scan for least.
export function agentHeaderChips(input: AgentChipsInput): HeaderChip[] {
  const chips: HeaderChip[] = []
  if (input.projectName) {
    chips.push({ kind: "project", label: "Project", value: input.projectName })
  }
  chips.push({
    kind: "agent",
    label: "Agent",
    value: input.name,
    primary: input.primary !== "none",
  })
  const branch = branchChip(input)
  if (branch) chips.push(branch)
  const count = input.terminalCount ?? 0
  if (count > 0) {
    chips.push({ kind: "terminal", label: "Terminals", value: String(count) })
  }
  chips.push({
    kind: "assistant",
    label: "Assistant",
    value: input.provider,
    hint: ASSISTANT_HINT,
  })
  return chips
}

// The chip for a FOCUSED terminal: the terminal is what is on screen, so it is
// the primary chip and the last to give way. Its value is the terminal's title
// (the foreground command when one is running, else the stable "Terminal N"),
// and the owner's sibling count moves into the hover clause rather than being
// dropped, since the title has taken the chip's text.
export function focusedTerminalChip(
  title: string,
  siblingCount: number,
): HeaderChip {
  const count = siblingCount > 1 ? terminalCountCaption(siblingCount) : null
  return {
    kind: "terminal",
    label: "Terminal",
    value: title,
    hint: count ?? undefined,
    primary: true,
  }
}

// The chip naming a STANDALONE terminal's directory. A standalone terminal has
// no owner to name, so where it is IS its context. It reuses the folder glyph:
// a directory is a folder, and the two never appear together (a standalone
// terminal belongs to no project), so the shared glyph never has to mean two
// things at once in one row.
export function directoryChip(cwdLabel: string): HeaderChip {
  return { kind: "directory", label: "Directory", value: cwdLabel }
}

// The chips the PHONE header may draw, in the order the desktop row would put
// them. Everything else the desktop carries (the branch, the terminal count) is
// dropped here, and that is the honest cost of glyph labels rather than an
// omission: a glyph is learnable only through its hover, there is no hover on a
// phone, so the phone keeps the two fields that matter most beside the name and
// shows no glyph nobody can interrogate.
const PHONE_CHIP_KINDS: readonly HeaderChipKind[] = [
  "project",
  "agent",
  "assistant",
]

// The phone header's two lanes, derived from the SAME chip model the desktop
// row renders so a label or a value can never drift between the surfaces.
//
// Lane one is the primary chip (the agent name, the one value that runs long
// and the thing you navigate by) at full size. Lane two is what is left, at
// 11px muted: project, then assistant. The old mobile header showed the branch
// alone, in mono, so it never said which project or which assistant you were
// talking to, and on an agent named after its branch it repeated the word the
// sidebar row had just said.
export function mobileHeaderLanes(input: AgentChipsInput): {
  lead: HeaderChip
  rest: HeaderChip[]
} {
  const chips = agentHeaderChips(input).filter((c) =>
    PHONE_CHIP_KINDS.includes(c.kind),
  )
  // The agent chip is always produced by `agentHeaderChips`, and on the phone
  // it is always the primary one (a terminal has its own phone screen), so this
  // find cannot miss; the fallback keeps the return type honest anyway.
  const lead = chips.find((c) => c.kind === "agent") ?? chips[0]
  return { lead, rest: chips.filter((c) => c !== lead) }
}
