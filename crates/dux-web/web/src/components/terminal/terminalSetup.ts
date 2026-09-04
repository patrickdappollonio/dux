import type { RefObject } from "react"
import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"

import { registerAgentNotifications } from "@/lib/agentNotifications"
import { inactiveCursorStyle } from "@/lib/composebar"
import { isApplePlatform } from "@/lib/platform"
import { suppressViewerReports } from "@/lib/suppressViewerReports"
import {
  clampTerminalFontSize,
  loadTerminalFontsThenRefit,
  terminalFontFamily,
} from "@/lib/terminalFont"

import {
  WHEEL_SCROLL_SENSITIVITY,
  xtermScrollbarWidth,
} from "./constants"
import { createLinkPress, type LinkPress } from "./linkPress"
import type { LiveSettings } from "./liveValues"
import type { ResizeCoordinator } from "./resizeCoordinator"
import { attachWebglRenderer } from "./webglRenderer"

type Disposable = { dispose: () => void }

export type TerminalSetup = {
  term: Terminal
  links: LinkPress
  isMac: boolean
  fontFamily: string
  fontSize: number
  disposeAgentNotifications: () => void
  disposeOsc8Gate: Disposable
  /// The webgl renderer, when this browser got one. Written by `openTerminal`
  /// (the addon needs an open terminal) and released by `disposeTerminalSetup`;
  /// null on every DOM-renderer rung of the ladder in `webglRenderer.ts`.
  renderer: Disposable | null
}

type CreateTerminalSetupOptions = {
  host: HTMLDivElement
  id: string
  live: LiveSettings
}

function resolveTerminalBackground(): string {
  const rawBackground = getComputedStyle(document.documentElement)
    .getPropertyValue("--background")
    .trim()
  let resolvedBackground = "#000000"
  try {
    const canvas = document.createElement("canvas")
    canvas.width = 1
    canvas.height = 1
    const context = canvas.getContext("2d")
    if (context && rawBackground) {
      context.fillStyle = `oklch(${rawBackground})`
      context.fillRect(0, 0, 1, 1)
      const [red, green, blue] = context.getImageData(0, 0, 1, 1).data
      resolvedBackground = `#${red.toString(16).padStart(2, "0")}${green.toString(16).padStart(2, "0")}${blue.toString(16).padStart(2, "0")}`
    }
  } catch {
    // Canvas color resolution is optional; black is the safe xterm fallback.
  }
  return resolvedBackground
}

export function createTerminalSetup(
  options: CreateTerminalSetupOptions,
): TerminalSetup {
  const { host, id, live } = options
  const background = resolveTerminalBackground()
  host.style.background = background

  const scrollbarWidth = xtermScrollbarWidth()
  const isMac = isApplePlatform()
  const links = createLinkPress({
    hyperlinks: () => live.current.hyperlinks,
    isMac,
  })
  const fontFamily = terminalFontFamily(live.current.fontFamily)
  const fontSize = clampTerminalFontSize(live.current.fontSize)
  const term = new Terminal({
    fontFamily,
    fontSize,
    cursorBlink: true,
    cursorInactiveStyle: inactiveCursorStyle(live.current.composeActive),
    convertEol: false,
    scrollback: live.current.scrollbackLines,
    scrollSensitivity: WHEEL_SCROLL_SENSITIVITY,
    overviewRuler: { width: scrollbarWidth },
    theme: { background },
    macOptionClickForcesSelection: true,
    linkHandler: links.linkHandler,
  })
  links.setTerminal(term)
  suppressViewerReports(term)

  const disposeAgentNotifications = registerAgentNotifications(term, {
    enabled: () => live.current.webNotifications,
    title: () => live.current.notifyTitle,
    clipboardMode: () => live.current.clipboardPassthrough,
    tag: () => `dux-agent-${id}`,
  })
  const disposeOsc8Gate = term.parser.registerOscHandler(
    8,
    () => !live.current.hyperlinks,
  )

  return {
    term,
    links,
    isMac,
    fontFamily,
    fontSize,
    disposeAgentNotifications,
    disposeOsc8Gate,
    renderer: null,
  }
}

type OpenTerminalOptions = {
  setup: TerminalSetup
  container: HTMLDivElement
  fit: FitAddon
  resize: ResizeCoordinator
  termRef: RefObject<Terminal | null>
  fitAddonRef: RefObject<FitAddon | null>
  noteLocalGrid: (grid: { rows: number; cols: number }) => void
}

export function openTerminal(options: OpenTerminalOptions): Disposable {
  const {
    setup,
    container,
    fit,
    resize,
    termRef,
    fitAddonRef,
    noteLocalGrid,
  } = options
  const { term, fontFamily, fontSize } = setup

  term.open(container)
  if (term.textarea) {
    term.textarea.setAttribute("autocomplete", "off")
    term.textarea.setAttribute("autocorrect", "off")
    term.textarea.setAttribute("autocapitalize", "off")
    term.textarea.setAttribute("spellcheck", "false")
  }
  // BEFORE the first fit, so the grid is measured against the renderer that
  // will actually paint it. Null on every browser the ladder in
  // `webglRenderer.ts` keeps on the DOM renderer, which is the behavior this
  // pane had before the addon existed.
  setup.renderer = attachWebglRenderer(term)
  resize.fitAfterOpen()

  noteLocalGrid({ rows: term.rows, cols: term.cols })
  const localGridSubscription = term.onResize(({ rows, cols }) =>
    noteLocalGrid({ rows, cols }),
  )
  termRef.current = term
  fitAddonRef.current = fit
  loadTerminalFontsThenRefit(
    term,
    termRef,
    () => resize.refitForFonts(),
    fontSize,
    fontFamily,
  )
  return localGridSubscription
}

export function disposeTerminalSetup(setup: TerminalSetup): void {
  setup.disposeAgentNotifications()
  setup.disposeOsc8Gate.dispose()
  // The GPU context goes back BEFORE the terminal does. `term.dispose()` would
  // release the addon too (xterm disposes what `loadAddon` registered), but a
  // browser caps live WebGL contexts per page and drops the oldest when the cap
  // is hit, so the pane that closes hands its context back explicitly rather
  // than relying on someone else's teardown order.
  setup.renderer?.dispose()
  setup.renderer = null
  setup.term.dispose()
}
