// Shared GitHub PR state to presentation mapping, the one intentional semantic-color exception
// in the web UI: open is green, draft GitHub's draft gray, merged purple, closed red, matching
// the TUI banner. The sidebar
// icon and the PR banner lane both read from here so the two surfaces cannot drift.

import type { PrKeyView, PrView } from "@/lib/types"

// A PR state plus a defensive "unknown" bucket for a value the server may add later, so a
// caller falls back to a neutral presentation rather than crashing.
type PrStateLike = PrView["state"] | (string & {})

// The human word for a state, mirroring the TUI's lowercase wording
// ("open"/"draft"/"merged"/"closed"). Anything unrecognized reads as "unknown".
export function prStateLabel(state: PrStateLike): string {
  if (state === "open") return "open"
  if (state === "draft") return "draft"
  if (state === "merged") return "merged"
  if (state === "closed") return "closed"
  return "unknown"
}

// The sentence a screen reader speaks. "Pull request" is spelled out, since the abbreviation
// is read as letters, and an unrecognized state is left off rather than spoken as "unknown".
export function prAriaLabel(number: number, state: PrStateLike): string {
  const label = prStateLabel(state)
  return label === "unknown"
    ? `Pull request #${number}`
    : `Pull request #${number}, ${label}`
}

// Icon-only tint for the sidebar/mobile session row: just the glyph color, no
// background (the row carries its own hover/active surface).
export function prIconClass(state: PrStateLike): string {
  if (state === "open") return "text-dux-pr-open"
  if (state === "draft") return "text-dux-pr-draft"
  if (state === "merged") return "text-dux-pr-merged"
  if (state === "closed") return "text-dux-pr-closed"
  return "text-muted-foreground"
}

// Explicit hover for the icon link: the row's near-white hover surface washes a plain glyph
// out, so a brighter glyph plus a subtle same-hue chip keeps it readable on both surfaces.
export function prIconHoverClass(state: PrStateLike): string {
  if (state === "open") return "hover:bg-dux-pr-open-tint/15 hover:text-dux-pr-open-hover"
  if (state === "draft") return "hover:bg-dux-pr-draft-tint/15 hover:text-dux-pr-draft-hover"
  if (state === "merged") return "hover:bg-dux-pr-merged-tint/15 hover:text-dux-pr-merged-hover"
  if (state === "closed") return "hover:bg-dux-pr-closed-tint/15 hover:text-dux-pr-closed-hover"
  return "hover:bg-muted hover:text-foreground"
}

// Soft state-colored strip for the PR banner lane: tinted background + matching
// border + readable text, mirroring the TUI banner's green/gray/purple/red fill.
// It carries no hover of its own, because only the strip's anchor is the link
// and its trailing controls are not.
export function prBannerClass(state: PrStateLike): string {
  if (state === "open") return "bg-dux-pr-open-tint/10 border-dux-pr-open-tint/30 text-dux-pr-open"
  if (state === "draft")
    return "bg-dux-pr-draft-tint/10 border-dux-pr-draft-tint/30 text-dux-pr-draft"
  if (state === "merged")
    return "bg-dux-pr-merged-tint/10 border-dux-pr-merged-tint/30 text-dux-pr-merged"
  if (state === "closed")
    return "bg-dux-pr-closed-tint/10 border-dux-pr-closed-tint/30 text-dux-pr-closed"
  return "bg-muted border-border text-muted-foreground"
}

// The banner anchor's hover, laid over the strip's own tint so the link reads
// a step stronger than the controls beside it.
export function prBannerLinkHoverClass(state: PrStateLike): string {
  if (state === "open") return "hover:bg-dux-pr-open-tint/5"
  if (state === "draft") return "hover:bg-dux-pr-draft-tint/5"
  if (state === "merged") return "hover:bg-dux-pr-merged-tint/5"
  if (state === "closed") return "hover:bg-dux-pr-closed-tint/5"
  return "hover:bg-muted/80"
}

// Every pull request an agent shows, in its order. A server that predates the
// list sends only the shown one.
export function sessionPrs(session: { pr?: PrView; prs?: PrView[] }): PrView[] {
  return session.prs ?? (session.pr ? [session.pr] : [])
}

// How a pull request is named: `#418` in the agent's own repository,
// `acme/infra#77` from any other. A server that does not say which repository
// is the agent's own names every one by number, as it always did.
export function prName(pr: PrView): string {
  return pr.own_repo === false && pr.owner_repo
    ? `${pr.owner_repo}#${pr.number}`
    : `#${pr.number}`
}

// The key a change names a pull request by, or null when the server did not
// send one (an older server, which has no per-pull-request routes either).
export function prKeyOf(pr: PrView): PrKeyView | null {
  return pr.host && pr.owner_repo
    ? { host: pr.host, owner_repo: pr.owner_repo, number: pr.number }
    : null
}

export function samePrKey(a: PrKeyView, b: PrKeyView): boolean {
  return a.number === b.number && a.host === b.host && a.owner_repo === b.owner_repo
}

// Where a stepping cursor that names `key` sits in `prs`: on that pull request
// while it is in the list, and back on the first once it has left.
export function prCursorIndex(prs: readonly PrView[], key: PrKeyView | null): number {
  if (!key) return 0
  const at = prs.findIndex((pr) => {
    const own = prKeyOf(pr)
    return own !== null && samePrKey(own, key)
  })
  return at < 0 ? 0 : at
}

// One step through a list of `len`, wrapping at both ends.
export function stepPrIndex(len: number, from: number, dir: 1 | -1): number {
  if (len <= 0) return 0
  return (((from + dir) % len) + len) % len
}

// What a row offers to take a pull request out of sight: one the user attached
// is detached (it then stays as found when dux also found it), and one dux
// only found is hidden.
export function prRowAction(pr: PrView): "detach" | "hide" {
  return (pr.attached ?? pr.overridden) ? "detach" : "hide"
}

// The second line's origin: the branch dux found it on, or that the user
// attached it. Null for a found one whose branch GitHub has not named yet.
export type PrOrigin = { kind: "branch"; name: string } | { kind: "attached" }

export function prOrigin(pr: PrView): PrOrigin | null {
  const found = pr.found ?? !(pr.attached ?? pr.overridden)
  if (found && pr.head_branch) return { kind: "branch", name: pr.head_branch }
  if (pr.attached ?? pr.overridden) return { kind: "attached" }
  return null
}

const RELATIVE_STEPS: readonly [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 86_400_000],
  ["month", 30 * 86_400_000],
  ["week", 7 * 86_400_000],
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
]

function relativeWords(iso: string, now: number): string | null {
  const then = Date.parse(iso)
  if (Number.isNaN(then)) return null
  const delta = then - now
  const format = new Intl.RelativeTimeFormat("en", { numeric: "auto" })
  for (const [unit, ms] of RELATIVE_STEPS) {
    if (Math.abs(delta) >= ms) return format.format(Math.round(delta / ms), unit)
  }
  return "just now"
}

// The relative time a row ends on: when a merged one merged, a closed one
// closed, and anything else was last updated. Null when GitHub gave no time.
export function prActivity(pr: PrView, now: number = Date.now()): string | null {
  const [verb, at] =
    pr.state === "merged"
      ? ["merged", pr.merged_at]
      : pr.state === "closed"
        ? ["closed", pr.closed_at]
        : ["updated", pr.updated_at]
  if (!at) return null
  const words = relativeWords(at, now)
  return words ? `${verb} ${words}` : null
}

// The row badge's tooltip once an agent has more than one pull request: the
// shown one, and how many more the banner steps through.
export function prBadgeTooltip(prs: readonly PrView[]): string {
  const [shown] = prs
  const more = prs.length - 1
  const noun = more === 1 ? "pull request" : "pull requests"
  return `${prName(shown)} ${prStateLabel(shown.state)} · ${shown.title}. ${more} more ${noun}; the banner steps through them.`
}
