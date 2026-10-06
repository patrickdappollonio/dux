// Who a destructive change would cut off, read from the server's `409 attached`
// refusal. dux-core's `attached_prose` builds the same sentences; prose_cross_language.json pins both.

import { sessionLabel } from "./agentWorkspace"
import { tabProseLabel } from "./agentTabs"
import { deviceLabel } from "./deviceLabel"
import { chip, type Prose, type ProseSegment } from "./prose"
import type { SessionView, TerminalView } from "./types"

/** One attachment in the way, as the server sends it. */
export interface AttachedBlocker {
  surface: "browser" | "terminal_ui"
  /** The raw `User-Agent`, or the terminal UI's fixed label. */
  device: string | null
  address: string | null
  verified: boolean
  /** Typing in it rather than watching it. */
  driving: boolean
  target: { kind: "tab" | "terminal"; id: string; agent?: string }
  /** Who this is, sent back to go ahead over exactly the blockers shown. */
  key: string
}

/** A change the server refused because somebody else is attached to what it
 * would end. Nothing was changed. */
export class AttachedError extends Error {
  readonly status = 409
  readonly blockers: AttachedBlocker[]
  constructor(blockers: AttachedBlocker[]) {
    super(
      "Someone else is using this right now, so dux did not go ahead and nothing was changed.",
    )
    this.name = "AttachedError"
    this.blockers = blockers
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null
}

function blockerFrom(value: unknown): AttachedBlocker | null {
  if (!isRecord(value) || !isRecord(value.target)) return null
  const { target } = value
  if (target.kind !== "tab" && target.kind !== "terminal") return null
  if (typeof target.id !== "string") return null
  return {
    surface: value.surface === "terminal_ui" ? "terminal_ui" : "browser",
    device: typeof value.device === "string" ? value.device : null,
    address: typeof value.address === "string" ? value.address : null,
    verified: value.verified === true,
    driving: value.driving === true,
    target: {
      kind: target.kind,
      id: target.id,
      ...(typeof target.agent === "string" ? { agent: target.agent } : {}),
    },
    key: typeof value.key === "string" ? value.key : "",
  }
}

/** The query that goes ahead over exactly the blockers a dialog showed (their
 * keys); the server refuses again when anybody else is in the way by then. */
export function forceConnectedQuery(
  accepted: readonly string[] | null,
  first: boolean,
): string {
  if (accepted === null) return ""
  const keys = accepted.map(encodeURIComponent).join(",")
  return `${first ? "?" : "&"}force_connected=${keys}`
}

/** The blockers of an `attached` refusal, or `null` for any other answer. */
export function attachedBlockers(
  status: number,
  body: unknown,
): AttachedBlocker[] | null {
  if (status !== 409 || !isRecord(body) || body.error !== "attached") return null
  if (!Array.isArray(body.blockers)) return null
  return body.blockers
    .map(blockerFrom)
    .filter((b): b is AttachedBlocker => b !== null)
}

/** One blocker with every name resolved, the shape both surfaces' sentence
 * builders take. */
export interface AttachedEntry {
  device: string | null
  surface: "browser" | "terminal_ui"
  address: string | null
  verified: boolean
  driving: boolean
  targetKind: "tab" | "terminal"
  targetLabel: string
  agentLabel: string | null
}

/** The blockers with the names a dialog shows; a target that is gone shows its id. */
export function attachedEntries(
  blockers: readonly AttachedBlocker[],
  spine: { sessions: SessionView[]; terminals: TerminalView[] } | null,
): AttachedEntry[] {
  return blockers.map((blocker) => {
    const { target } = blocker
    const session = target.agent
      ? spine?.sessions.find((s) => s.id === target.agent)
      : undefined
    const label =
      target.kind === "tab"
        ? session && tabProseLabel(session.tabs, target.id)
        : spine?.terminals.find((t) => t.id === target.id)?.label
    return {
      device: deviceLabel(blocker.device),
      surface: blocker.surface,
      address: blocker.address,
      verified: blocker.verified,
      driving: blocker.driving,
      targetKind: target.kind,
      targetLabel: label ?? target.id,
      agentLabel: session ? sessionLabel(session) : null,
    }
  })
}

/** The line above the list. */
export function attachedLeadProse(): Prose {
  return ["Someone else is using this right now. Going ahead cuts them off:"]
}

// The terminal UI's fixed device label, for a terminal UI blocker that sent none.
const TUI_DEVICE_LABEL = "the dux TUI"

/** One line of the list: who, from where, typing or watching, and in what. */
export function attachedEntryProse(entry: AttachedEntry): Prose {
  const out: ProseSegment[] = []
  if (entry.device !== null) out.push(chip(entry.device))
  else if (entry.surface === "terminal_ui") out.push(chip(TUI_DEVICE_LABEL))
  else out.push("a browser")
  let words = ""
  if (entry.address !== null) {
    words += ` at ${entry.address}`
    if (!entry.verified) words += " (unverified)"
  }
  words += entry.driving ? ", typing in " : ", watching "
  words += entry.targetKind === "tab" ? "tab " : "terminal "
  out.push(words, chip(entry.targetLabel))
  if (entry.agentLabel !== null) out.push(" of agent ", chip(entry.agentLabel))
  return out
}
