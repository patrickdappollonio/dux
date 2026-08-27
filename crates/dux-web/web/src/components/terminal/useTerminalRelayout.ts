import { useLayoutEffect, useRef, type RefObject } from "react"
import type { FitAddon } from "@xterm/addon-fit"
import type { Terminal } from "@xterm/xterm"

import {
  clampTerminalFontSize,
  loadTerminalFontsThenRefit,
  terminalFontFamily,
} from "@/lib/terminalFont"
import { viewerFontFit } from "@/lib/viewerFit"

import { xtermScrollbarWidth } from "./constants"

type TerminalRelayoutOptions = {
  hostRef: RefObject<HTMLDivElement | null>
  containerRef: RefObject<HTMLDivElement | null>
  termRef: RefObject<Terminal | null>
  fitAddonRef: RefObject<FitAddon | null>
  viewerRegridRef: RefObject<(() => void) | null>
  setViewerOverflow: (overflow: boolean) => void
  fontFamilySetting: string
  fontSizeSetting: number
  faithfulWatcher: boolean
  remoteRows: number
  remoteCols: number
}

type RelayoutOptions = TerminalRelayoutOptions & {
  viewerRelayoutRef: RefObject<(() => void) | null>
}

type MountedTerminal = Pick<
  TerminalRelayoutOptions,
  | "containerRef"
  | "termRef"
  | "fitAddonRef"
  | "viewerRegridRef"
  | "setViewerOverflow"
> & {
  host: HTMLDivElement
  container: HTMLDivElement
  term: Terminal
  viewerRelayoutRef: RefObject<(() => void) | null>
}

function clearViewerOverflow(context: MountedTerminal): void {
  context.setViewerOverflow(false)
  context.container.style.removeProperty("width")
  context.container.style.removeProperty("height")
}

function measuredCell(term: Terminal): { width: number; height: number } {
  const screen = term.element?.querySelector(".xterm-screen")
  const rect = screen?.getBoundingClientRect()
  if (!rect || term.cols <= 0 || term.rows <= 0) {
    return { width: 0, height: 0 }
  }
  return { width: rect.width / term.cols, height: rect.height / term.rows }
}

function hostContentSize(
  host: HTMLDivElement,
  gutter: number,
): { width: number; height: number } {
  const style = getComputedStyle(host)
  const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight)
  const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
  return {
    width: host.clientWidth - padX - gutter,
    height: host.clientHeight - padY,
  }
}

function faithfulFontSize(
  context: MountedTerminal,
  rows: number,
  cols: number,
  preferredSize: number,
): number {
  const gutter = xtermScrollbarWidth()
  const fitted = viewerFontFit({
    available: hostContentSize(context.host, gutter),
    grid: { rows, cols },
    cell: measuredCell(context.term),
    referenceFontSize:
      typeof context.term.options.fontSize === "number"
        ? context.term.options.fontSize
        : preferredSize,
    maxFontSize: preferredSize,
  })
  if (!fitted.overflows) {
    clearViewerOverflow(context)
    return fitted.fontSize
  }

  context.setViewerOverflow(true)
  context.container.style.width = `${fitted.width + gutter}px`
  context.container.style.height = `${fitted.height}px`
  return fitted.fontSize
}

function applyFont(
  context: MountedTerminal,
  family: string,
  size: number,
  faithful: boolean,
  wasFaithful: boolean,
): void {
  const familyChanged = context.term.options.fontFamily !== family
  const sizeChanged = context.term.options.fontSize !== size
  if (familyChanged) context.term.options.fontFamily = family
  if (sizeChanged) context.term.options.fontSize = size

  if (faithful) {
    context.viewerRegridRef.current?.()
  } else if (familyChanged || sizeChanged || wasFaithful) {
    context.fitAddonRef.current?.fit()
  }

  if (!familyChanged) return
  loadTerminalFontsThenRefit(
    context.term,
    context.termRef,
    () => context.viewerRelayoutRef.current?.(),
    size,
    family,
  )
}

function mountedTerminal(
  options: RelayoutOptions,
): MountedTerminal | null {
  const host = options.hostRef.current
  const container = options.containerRef.current
  const term = options.termRef.current
  if (!host || !container || !term) return null
  return { ...options, host, container, term }
}

function relayoutTerminal(
  options: RelayoutOptions,
  wasFaithful: boolean,
): boolean {
  const context = mountedTerminal(options)
  if (!context) return wasFaithful

  const faithful =
    options.faithfulWatcher && options.remoteRows > 0 && options.remoteCols > 0
  const preferredSize = clampTerminalFontSize(options.fontSizeSetting)
  const size = faithful
    ? faithfulFontSize(
        context,
        options.remoteRows,
        options.remoteCols,
        preferredSize,
      )
    : preferredSize
  if (!faithful) clearViewerOverflow(context)
  applyFont(
    context,
    terminalFontFamily(options.fontFamilySetting),
    size,
    faithful,
    wasFaithful,
  )
  return faithful
}

export function useTerminalRelayout(
  options: TerminalRelayoutOptions,
): RefObject<(() => void) | null> {
  const lastFaithfulRef = useRef(false)
  const viewerRelayoutRef = useRef<(() => void) | null>(null)
  const {
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    viewerRegridRef,
    setViewerOverflow,
    fontFamilySetting,
    fontSizeSetting,
    faithfulWatcher,
    remoteRows,
    remoteCols,
  } = options

  useLayoutEffect(() => {
    const relayoutOptions = {
      hostRef,
      containerRef,
      termRef,
      fitAddonRef,
      viewerRegridRef,
      viewerRelayoutRef,
      setViewerOverflow,
      fontFamilySetting,
      fontSizeSetting,
      faithfulWatcher,
      remoteRows,
      remoteCols,
    }
    const relayout = () => {
      lastFaithfulRef.current = relayoutTerminal(
        relayoutOptions,
        lastFaithfulRef.current,
      )
    }
    viewerRelayoutRef.current = relayout
    relayout()
    return () => {
      if (viewerRelayoutRef.current === relayout) {
        viewerRelayoutRef.current = null
      }
    }
  }, [
    containerRef,
    faithfulWatcher,
    fitAddonRef,
    fontFamilySetting,
    fontSizeSetting,
    hostRef,
    remoteCols,
    remoteRows,
    setViewerOverflow,
    termRef,
    viewerRegridRef,
  ])
  return viewerRelayoutRef
}
