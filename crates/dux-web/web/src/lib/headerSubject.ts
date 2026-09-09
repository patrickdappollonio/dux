// The header metadata strip is one row of chips, each a glyph followed by its
// value. Which chips exist and what each says is decided here rather than in the
// components, so the agent and terminal variants cannot drift apart and the rule
// is testable without mounting React.
//
// The glyph replaces a written label and doubles as the separator, so the word
// it stands for is recovered on hover. That hover is the only thing that makes a
// glyph learnable, so every chip carries a non-empty `label`.

// Joins the clauses of a tooltip, and the phone header's one caption line. A
// middot rather than a hairline divider because it is one small run of text.
export const CAPTION_SEPARATOR = " · "

// The chips, in the order they render. `terminal` is one kind rather than two:
// on an agent it counts that agent's terminals, on a focused terminal it names
// the one you are looking at, and the glyph means the same thing in both.
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
  // An extra tooltip clause, after the label and after the value when the value
  // is cut off.
  hint?: string
  // Exactly one chip per header is primary: the thing you navigate by. It is the
  // last to give way when the row runs out of room.
  primary?: boolean
}

// The assistant chip's hover clause: the provider is not editable from the
// header, so the tooltip names where it is.
export const ASSISTANT_HINT = "change it in the agent menu"

// Drop empties and join, so callers pass optional parts straight through rather
// than filtering at each site.
export function captionText(
  parts: readonly (string | null | undefined)[],
): string {
  return parts.filter((p): p is string => !!p).join(CAPTION_SEPARATOR)
}

// What a chip says on hover: the label always, and the value only when it is cut
// off on screen, since repeating readable text is noise. Only the render can
// measure that, so the caller passes `truncated` in.
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
  // The agent's display name (its title, falling back to its branch, or to its
  // folder's name for a standalone agent).
  name: string
  provider: string
  projectName?: string | null
  // A standalone agent's folder, home-collapsed, and mutually exclusive with
  // `projectName`. It takes the same leading slot through `directoryChip`, the
  // chip a standalone terminal already uses, so the two stay one idiom.
  folderLabel?: string | null
  // The branch this agent tracks, or `null` when it has none (a standalone
  // agent). Null rather than an empty string so a missing branch cannot be
  // mistaken for a branch named "".
  branchName: string | null
  // The immutable branch the agent was created on. Absent on an older server,
  // and always absent for a standalone agent.
  initialBranch?: string | null
  // How many terminals this agent owns. Zero renders no terminal chip.
  terminalCount?: number
  // Set when a terminal is the thing on screen and this agent merely owns it.
  // The agent then stops being the primary chip and the terminal takes over.
  primary?: "agent" | "none"
}

// True when the branch has moved off the one the agent was created on. Guarded
// on `initialBranch`, which an older server omits.
function branchDrifted(input: AgentChipsInput): boolean {
  return (
    !!input.branchName &&
    !!input.initialBranch &&
    input.initialBranch !== input.branchName
  )
}

// The branch chip, or null. It is omitted where the branch merely repeats the
// agent name (an untitled agent takes its name from its branch), and appears
// when the two differ. It also appears on a drifted branch even when they match,
// because the drift note has nowhere else to live.
export function branchChip(input: AgentChipsInput): HeaderChip | null {
  // A standalone agent has no branch, so there is no chip: an empty one would
  // draw a glyph with nothing after it.
  if (!input.branchName) return null
  const drifted = branchDrifted(input)
  if (input.branchName === input.name && !drifted) return null
  return {
    kind: "branch",
    label: "Branch",
    value: input.branchName,
    hint: drifted ? `originally ${input.initialBranch}` : undefined,
  }
}

// The chips describing an agent, coarsest fact first: project, agent, branch,
// terminals, assistant. Branch and terminals appear only when they have
// something to say, which is what keeps the ordinary row short.
export function agentHeaderChips(input: AgentChipsInput): HeaderChip[] {
  const chips: HeaderChip[] = []
  if (input.projectName) {
    chips.push({ kind: "project", label: "Project", value: input.projectName })
  } else if (input.folderLabel) {
    // The standalone agent's answer to the same question, through the very
    // chip a standalone terminal uses for it.
    chips.push(directoryChip(input.folderLabel))
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

// The chip for a focused terminal: on screen, so it is the primary chip. Its
// value is the terminal's title, which pushes the owner's sibling count into the
// hover clause rather than dropping it.
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

// The chip naming a standalone terminal's directory, which is its whole context
// since it has no owner. It reuses the folder glyph, which is unambiguous
// because a directory chip and a project chip never appear in one row.
export function directoryChip(cwdLabel: string): HeaderChip {
  return { kind: "directory", label: "Directory", value: cwdLabel }
}

// The chips the phone header may draw, in the desktop row's order. The rest are
// dropped because a glyph is learnable only through its hover and a phone has
// none, so the phone shows no glyph nobody can interrogate.
const PHONE_CHIP_KINDS: readonly HeaderChipKind[] = [
  "project",
  // A standalone agent's answer to the project question, and the only thing on
  // a phone that says where it is working.
  "directory",
  "agent",
  "assistant",
]

// The phone header's two lanes, derived from the same chip model the desktop row
// renders so a label or a value cannot drift between the surfaces. Lane one is
// the primary chip at full size; lane two is what is left, muted.
export function mobileHeaderLanes(input: AgentChipsInput): {
  lead: HeaderChip
  rest: HeaderChip[]
} {
  const chips = agentHeaderChips(input).filter((c) =>
    PHONE_CHIP_KINDS.includes(c.kind),
  )
  // `agentHeaderChips` always produces the agent chip, so the fallback exists
  // only to keep the return type honest.
  const lead = chips.find((c) => c.kind === "agent") ?? chips[0]
  return { lead, rest: chips.filter((c) => c !== lead) }
}
