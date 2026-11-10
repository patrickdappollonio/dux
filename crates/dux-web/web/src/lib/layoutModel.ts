// Pure layout rules for an agent's stack: its provider-tab terminal (the
// "agent" part) above a dock of its companion terminals (the "dock" part). A
// computer shows the two as a vertical split; a phone shows one at a time. This
// module decides what the stack looks like and holds no state of its own: the
// caller owns the remembered values and passes them in.

import { shouldShowTabStrip } from "./agentTabs"
import type { AgentTabView } from "./types"

/** A computer's stack: agent maximized, both parts, or terminal maximized. */
export type ComputerLayout = "agent" | "split" | "terminal"

/** A phone shows one part of the stack at a time. */
export type PhoneView = "agent" | "terminals"

export type StackPart = "agent" | "dock"

/** The controls the dock strip and the floating pill offer on a computer. */
export type LayoutAction = "hideTerminals" | "maximizeTerminal" | "restore"

/**
 * The layout after `action`. An action that is not offered in `layout` (hiding
 * terminals that are already hidden, restoring a split) leaves it unchanged
 * rather than guessing what the press meant.
 */
export function applyLayoutAction(
  layout: ComputerLayout,
  action: LayoutAction,
): ComputerLayout {
  switch (layout) {
    case "split":
      if (action === "hideTerminals") return "agent"
      if (action === "maximizeTerminal") return "terminal"
      return layout
    case "agent":
    case "terminal":
      return action === "restore" ? "split" : layout
  }
}

/** The layout a stopped agent comes back in when it is started. */
export function layoutOnStart(
  remembered: ComputerLayout | null | undefined,
): ComputerLayout {
  return remembered ?? "split"
}

export interface DividerShares {
  /** The agent part's share of the stack height, in percent. */
  agentShare: number
  /** The dock's share of the stack height, in percent. */
  dockShare: number
  /** The smallest share the agent part may keep before it folds, in percent. */
  agentMin: number
  /** The smallest share the dock may keep before it folds, in percent. */
  dockMin: number
}

/**
 * The layout a divider position folds into, or null while both parts keep
 * their minimum and the stack stays split. Dragging a part below its minimum
 * folds it away and maximizes the other, so a squeezed part never renders at a
 * size too small to use. The dock is checked first, so a stack too short for
 * both minimums keeps the agent.
 */
export function foldForDivider(
  shares: DividerShares,
): Exclude<ComputerLayout, "split"> | null {
  if (shares.dockShare < shares.dockMin) return "agent"
  if (shares.agentShare < shares.agentMin) return "terminal"
  return null
}

/**
 * Whether a divider release's ratio is worth remembering. A release that
 * folded a part says "hide this", not "this is the split I want", and storing
 * it would make the next restore come back at a size that folds again.
 */
export function shouldPersistDividerRatio(
  fold: ComputerLayout | null,
): boolean {
  return fold === null
}

/**
 * Whether the stack's two strips (the agent's provider strip and the dock
 * strip) are shown. They show together or not at all, so the agent part and
 * the dock always line up under strips of the same height.
 */
export function shouldShowStackStrips(
  tabs: AgentTabView[],
  terminalCount: number,
  alwaysShow = false,
): boolean {
  return shouldShowTabStrip(tabs, alwaysShow) || terminalCount >= 2
}

export interface PhoneViewInputs {
  terminalCount: number
  /** The address names one of the agent's terminals, with or without a tab. */
  addressNamesTerminal: boolean
  /** The agent's first tab is not running. */
  agentStopped: boolean
  anyTerminalRunning: boolean
  remembered: PhoneView | null | undefined
}

/**
 * Which part a phone opens on. An address naming a terminal opens the
 * terminals even when that terminal is gone, so its not-found screen is what
 * shows rather than a quiet fallback to the agent. A stopped agent with a
 * terminal still running opens on the terminals, because that is the only part
 * with anything live in it.
 */
export function resolvePhoneView(inputs: PhoneViewInputs): PhoneView {
  if (inputs.addressNamesTerminal) return "terminals"
  if (inputs.terminalCount === 0) return "agent"
  if (inputs.agentStopped && inputs.anyTerminalRunning) return "terminals"
  return inputs.remembered ?? "agent"
}

/**
 * What the agent's tab on top is doing, as far as mounting it goes:
 *   - "live": its process is running, so a socket only attaches.
 *   - "startsOnSight": dormant, and the dormant-card rule says showing it
 *     starts it (a healthy first tab, as selecting an agent does today).
 *   - "dormant": dormant, and nothing that opens the stack may start it.
 */
export type AgentTabState = "live" | "startsOnSight" | "dormant"

/** The layout or phone view the stack is in, as resolved by the functions
 * above. They fall back to the agent alone when there is no dock, except for
 * an address naming a terminal that is gone, which opens the dock so its
 * not-found screen lands where the terminal would be. */
export type StackPresentation = (
  | { surface: "computer"; layout: ComputerLayout }
  | { surface: "phone"; view: PhoneView }
) & { agentTab: AgentTabState }

export type StackParts = Record<StackPart, boolean> & {
  /** Whether the agent part may hold a live socket. Subscribing to a dormant
   * tab's pty LAUNCHES it whether or not the pane is on screen, so a dormant
   * tab gets its card instead of a socket unless it is on screen and showing
   * it is what starts it. */
  agentLiveSocket: boolean
}

/**
 * Which parts are on screen. A part that is off screen may stay mounted, but
 * it raises no attention of its own and makes no first claim of a row: nobody
 * can see it, so it must not act as if somebody were looking at it.
 */
export function partsOnScreen(presentation: StackPresentation): StackParts {
  const shown = shownParts(presentation)
  const agentLiveSocket =
    presentation.agentTab === "live" ||
    (presentation.agentTab === "startsOnSight" && shown.agent)
  return { ...shown, agentLiveSocket }
}

function shownParts(
  presentation: StackPresentation,
): Record<StackPart, boolean> {
  if (presentation.surface === "phone") {
    const dock = presentation.view === "terminals"
    return { agent: !dock, dock }
  }
  switch (presentation.layout) {
    case "split":
      return { agent: true, dock: true }
    case "agent":
      return { agent: true, dock: false }
    case "terminal":
      return { agent: false, dock: true }
  }
}

export interface StackOpeningInputs {
  /** The terminal the address names, if it names one. */
  addressTerminal: string | null | undefined
  rememberedTerminal: string | null | undefined
  rememberedLayout: ComputerLayout | null | undefined
  /** The live dock terminals, in dock order. */
  dockTerminals: readonly string[]
  /** The agent's first tab is not running. */
  agentStopped: boolean
  anyTerminalRunning: boolean
}

export interface StackOpening {
  /** The dock terminal in front, or null when there is none to show. */
  activeTerminal: string | null
  /** The address named a terminal that is gone; the UI says so. */
  terminalNotFound: boolean
  layout: ComputerLayout
  /** What the remembered dock terminal should be after opening. */
  rememberTerminal: string | null
  /** A bare agent address resolved to a remembered terminal and should be
   * rewritten in place (replace, never push) to name it. */
  rewriteAddress: boolean
}

/**
 * How an agent's stack opens. The address is a position and wins over memory;
 * memory is a convenience, so a remembered terminal that is gone is forgotten
 * quietly while an address naming a gone terminal is reported, never quietly
 * redirected to another one.
 */
export function resolveStackOpening(inputs: StackOpeningInputs): StackOpening {
  const live = inputs.dockTerminals
  const named = inputs.addressTerminal ?? null
  const remembered =
    inputs.rememberedTerminal != null &&
    live.includes(inputs.rememberedTerminal)
      ? inputs.rememberedTerminal
      : null

  let activeTerminal: string | null
  let terminalNotFound = false
  let rewriteAddress = false
  let rememberTerminal: string | null
  if (named !== null) {
    terminalNotFound = !live.includes(named)
    activeTerminal = terminalNotFound ? null : named
    rememberTerminal = terminalNotFound ? remembered : named
  } else {
    activeTerminal = remembered ?? live[0] ?? null
    rewriteAddress = remembered !== null
    rememberTerminal = remembered
  }

  return {
    activeTerminal,
    terminalNotFound,
    layout: openingLayout(inputs, named !== null),
    rememberTerminal,
    rewriteAddress,
  }
}

// A named terminal opens at least the split even when it turned out to be
// gone, so the not-found screen lands where the terminal would have been.
function openingLayout(
  inputs: StackOpeningInputs,
  addressNamesTerminal: boolean,
): ComputerLayout {
  if (inputs.agentStopped && inputs.anyTerminalRunning) return "terminal"
  if (addressNamesTerminal) {
    return inputs.rememberedLayout === "terminal" ? "terminal" : "split"
  }
  if (inputs.dockTerminals.length === 0) return "agent"
  return inputs.rememberedLayout ?? "split"
}

/**
 * The part typing goes to: the one chosen, while it is still a part of the
 * stack and on screen, else the first part on screen, else none: a part off
 * screen is never where typing goes, so with nothing typeable on screen no part
 * takes focus or renders input rows, and the chosen target stays. One
 * rule, read by the stack that routes the keystrokes and by the agent's screen
 * that hands the answer back to the store, so the header's controls, the top
 * menu and the stack never disagree about where typing goes.
 */
export function stackTypingTarget(
  parts: readonly { id: string; onScreen?: boolean }[],
  chosen: string | null,
): string | null {
  const onScreen = parts.filter((part) => part.onScreen !== false)
  if (chosen !== null && onScreen.some((part) => part.id === chosen)) return chosen
  return onScreen[0]?.id ?? null
}
