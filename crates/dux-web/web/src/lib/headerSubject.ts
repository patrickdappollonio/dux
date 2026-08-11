// The header metadata strip is "one subject plus a caption", and the decision of
// WHAT goes in each half lives here rather than in the components, so both the
// desktop `InsetHeader` and the phone header in `MobileShell` answer it the same
// way and so it can be unit-tested without mounting React.
//
// The shape it replaced was four labelled pairs (`agent: X | provider: Y |
// project: Z | branch: W`). Two measured problems drove the redesign: the labels
// were 34 of about 74 characters, so nearly half the bar spelled out what the
// values already made obvious, and an agent named after its branch (the common
// case, `server-mode` on `server-mode`) printed that one word twice.
//
// So: the SUBJECT is the thing you are looking at, unlabelled, at normal size,
// and everything else is a small muted caption beside it. The branch collapses
// to `same branch` when it merely repeats the subject.

// The caption's parts are joined by a middot. A middot rather than a hairline
// divider because the caption is one small run of text, not a row of equals.
export const CAPTION_SEPARATOR = " · "

// What the caption says instead of repeating the agent name when the branch and
// the agent name are the same string.
export const SAME_BRANCH_CAPTION = "same branch"

export interface HeaderSubject {
  // The unlabelled, foreground-coloured thing being named. Truncates LAST.
  subject: string
  // Small muted clauses beside it, in order. Truncates FIRST.
  caption: string[]
}

// Drop empties and join. Callers pass optional parts straight through (a project
// that could not be resolved, a terminal count of zero) rather than filtering at
// each site.
export function captionText(
  parts: readonly (string | null | undefined)[],
): string {
  return parts.filter((p): p is string => !!p).join(CAPTION_SEPARATOR)
}

// The branch clause. `same branch` when the branch merely repeats the name the
// header already shows, the branch itself otherwise. Compared against the NAME
// the header displays (an agent's title falls back to its branch, so an untitled
// agent collapses too, which is exactly the duplicated-word case).
export function branchCaption(subjectName: string, branchName: string): string {
  return branchName === subjectName ? SAME_BRANCH_CAPTION : branchName
}

export interface AgentCaptionInput {
  // The agent's display name (its title, falling back to its branch).
  name: string
  provider: string
  projectName?: string | null
  branchName: string
  // The immutable branch the agent was created on. Absent on an older server.
  initialBranch?: string | null
}

// The caption clauses for an AGENT, in order: project, provider, branch clause,
// and the drift clause when the current branch has moved off the one the agent
// was created on. Project leads because it is the coarsest fact and the one that
// answers "which codebase am I in"; provider follows because it answers "who am
// I talking to"; the branch clause is last because it is the one that usually
// says nothing new.
export function agentCaption(input: AgentCaptionInput): string[] {
  const parts: string[] = []
  if (input.projectName) parts.push(input.projectName)
  parts.push(input.provider)
  parts.push(branchCaption(input.name, input.branchName))
  if (input.initialBranch && input.initialBranch !== input.branchName) {
    parts.push(`originally ${input.initialBranch}`)
  }
  return parts
}

// The subject/caption pair for a focused AGENT. The agent's own name is the
// subject; everything else is caption.
export function agentHeaderSubject(input: AgentCaptionInput): HeaderSubject {
  return { subject: input.name, caption: agentCaption(input) }
}

// The phone header's second line: the two facts the old mobile header dropped
// entirely (it showed the branch and nothing else, so it never said which
// project or which assistant you were talking to). Deliberately NOT the full
// agent caption: a phone header has one short line to spend and the branch is
// the clause most likely to repeat the name above it.
export function mobileCaption(input: {
  provider: string
  projectName?: string | null
}): string {
  return captionText([input.projectName, input.provider])
}

// The sibling-count clause, pluralized. Omitted (null) at zero so callers can
// pass the count unconditionally.
export function terminalCountCaption(count: number): string | null {
  if (count <= 0) return null
  return count === 1 ? "1 terminal" : `${count} terminals`
}

// The subject/caption pair for a focused TERMINAL. The terminal is what is on
// screen, so it is the subject; its OWNER (an agent's caption, a project name, a
// standalone terminal's directory) becomes the caption in front of the sibling
// count. Same shape as the agent case, just a different subject.
export function terminalHeaderSubject(
  title: string,
  ownerCaption: readonly string[],
  siblingCount: number,
): HeaderSubject {
  return {
    subject: title,
    caption: [...ownerCaption, terminalCountCaption(siblingCount)].filter(
      (p): p is string => !!p,
    ),
  }
}
