import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react"
import { createPortal } from "react-dom"

import { AuthBanners } from "@/components/AuthBanners"
import {
  BlockedPage,
  BrokenPage,
  CheckingPage,
  LoginPage,
  StuckPage,
  UnreachablePage,
} from "@/components/LoginPage"
import { Toaster } from "@/components/ui/sonner"
import type { AuthStatus } from "@/lib/authApi"
import { assertNever } from "@/lib/assertNever"
import { useAuthPhase, type AuthPhase } from "@/lib/authGate"

// The root of the page: the app, and over it whatever page the sign-in gate
// needs (the login, blocked, broken, unreachable or stuck page).
//
// The app mounts the first time the gate opens and then STAYS mounted. A
// session that ends mid-use hides it (`visibility: hidden`, so its layout is
// kept and no terminal re-measures itself) and makes it `inert`, with the gate
// page as a full-page layer above it. Monaco models and their undo history,
// open dialogs and what is typed in them, drafts and the URL all survive the
// sign-in that follows. Everything that portals out of the app (dialogs,
// popups) is made inert alongside it, and the PTY sockets send nothing while
// signed out (`ptySocket.ts`), so no keystroke or resize reaches a terminal
// from a page nobody is signed in to.
//
// The gate page is its own layer, a direct child of the body rather than part
// of the app's root, for three reasons:
// - a modal open in the app when the session ended has hidden every other body
//   child from assistive tech (`aria-hidden` on the app's root); a layer added
//   after it is not marked, so the login page stays audible;
// - the same modal's outside-press dismissal ignores an element added after it
//   opened, so a press on the login page does not close it and throw away
//   what was typed in it;
// - key, pointer and focus events stop at the layer, so the app's document and
//   window listeners (a dialog's Escape, the sidebar's Ctrl/Cmd-B, theater's
//   Escape) never see what is typed on the login page. The layer's own
//   handlers run before that, on the layer itself.
//
// The toaster lives here, outside the app, so a toast on screen (a sticky one
// above all) outlives whatever the gate does.

// How long the first look may take before the page says what it is doing.
// Short enough that nobody stares at a blank page, long enough that a fast
// answer never flashes a message.
export const CHECKING_MESSAGE_DELAY_MS = 1500

function DelayedChecking() {
  const [show, setShow] = useState(false)
  useEffect(() => {
    const timer = setTimeout(() => setShow(true), CHECKING_MESSAGE_DELAY_MS)
    return () => clearTimeout(timer)
  }, [])
  return show ? <CheckingPage /> : <div className="min-h-svh bg-background" aria-busy="true" />
}

function gatePage(phase: Exclude<AuthPhase, { kind: "open" }>): ReactNode {
  switch (phase.kind) {
    case "checking":
      return <DelayedChecking />
    case "signed_out":
      return <LoginPage status={phase.status} reason={phase.reason} />
    case "blocked":
      return <BlockedPage where={phase.where} />
    case "broken":
      return <BrokenPage detail={phase.detail} />
    case "unreachable":
      return <UnreachablePage timedOut={phase.timedOut} />
    case "stuck":
      return <StuckPage />
    default:
      return assertNever(phase)
  }
}

// Events that stop at the gate layer instead of reaching the app's document and
// window listeners.
const CONTAINED_EVENTS = [
  "keydown",
  "keyup",
  "keypress",
  "pointerdown",
  "pointerup",
  "mousedown",
  "mouseup",
  "click",
  "dblclick",
  "contextmenu",
  "touchstart",
  "touchend",
  "focusin",
  "focusout",
  "paste",
  "copy",
  "cut",
  "wheel",
] as const

const GATE_LAYER_ATTR = "data-auth-gate-layer"

// The layer's host, a direct child of the body, made on first use and kept.
// Attached before anything renders into it, so a field's autofocus lands.
let gateHost: HTMLDivElement | null = null

function attachedGateHost(): HTMLDivElement {
  if (gateHost === null) {
    gateHost = document.createElement("div")
    gateHost.setAttribute(GATE_LAYER_ATTR, "")
  }
  if (!gateHost.isConnected) document.body.appendChild(gateHost)
  return gateHost
}

function GateLayer({ children }: { children: ReactNode }) {
  const host = attachedGateHost()
  useLayoutEffect(() => {
    // Nothing above the layer may hide it: whatever a modal marked while the
    // layer was empty is taken off the layer and its ancestors.
    for (let el: HTMLElement | null = host; el; el = el.parentElement) {
      if (el.getAttribute("aria-hidden") === "true") el.removeAttribute("aria-hidden")
      el.removeAttribute("data-base-ui-inert")
      el.removeAttribute("inert")
    }
    const stop = (e: Event) => e.stopPropagation()
    for (const type of CONTAINED_EVENTS) host.addEventListener(type, stop)
    return () => {
      for (const type of CONTAINED_EVENTS) host.removeEventListener(type, stop)
    }
  }, [host])
  return createPortal(
    // Fixed and scrollable on every load, so a landscape phone or an open
    // soft keyboard can still reach the button below the field.
    <div className="fixed inset-0 z-[200] overflow-y-auto bg-background">{children}</div>,
    host,
  )
}

// While `active`, everything in the body outside `root` (the portals the app
// opened) is inert, including anything that appears meanwhile, except the gate
// layer itself; what this set, it takes back.
function useInertOutside(root: React.RefObject<HTMLDivElement | null>, active: boolean): void {
  useEffect(() => {
    if (!active) return
    const self = root.current
    const marked = new Set<Element>()
    const mark = (el: Element) => {
      if (self !== null && el.contains(self)) return
      if (el.hasAttribute(GATE_LAYER_ATTR)) return
      if (el.hasAttribute("inert")) return
      el.setAttribute("inert", "")
      marked.add(el)
    }
    for (const el of Array.from(document.body.children)) mark(el)
    const observer =
      typeof MutationObserver === "undefined"
        ? null
        : new MutationObserver((records) => {
            for (const r of records) {
              for (const n of Array.from(r.addedNodes)) if (n instanceof Element) mark(n)
            }
          })
    observer?.observe(document.body, { childList: true })
    return () => {
      observer?.disconnect()
      for (const el of marked) el.removeAttribute("inert")
    }
  }, [root, active])
}

export function AuthGate({ children }: { children: ReactNode }) {
  const phase = useAuthPhase()
  const open = phase.kind === "open"
  // Render-time state updates, the documented way to derive from a prop that
  // moved: the app mounts on the first open and never unmounts, and the
  // banners keep the last status an open page had while it is hidden, so the
  // layout under the gate page does not shift.
  const [everOpen, setEverOpen] = useState(false)
  if (open && !everOpen) setEverOpen(true)
  const [lastStatus, setLastStatus] = useState<AuthStatus | null>(null)
  if (open && phase.status !== lastStatus) setLastStatus(phase.status)

  const rootRef = useRef<HTMLDivElement>(null)
  useInertOutside(rootRef, everOpen && !open)

  return (
    <div ref={rootRef}>
      {everOpen ? (
        <div
          data-testid="app-shell"
          inert={!open}
          aria-hidden={open ? undefined : true}
          style={open ? undefined : { visibility: "hidden" }}
        >
          <AuthBanners status={open ? phase.status : lastStatus} />
          {children}
        </div>
      ) : null}
      {/* Above every app layer, the offline overlay's included: while the
          gate is up, it is the only thing the page is about. */}
      {open ? null : <GateLayer>{gatePage(phase)}</GateLayer>}
      <Toaster />
    </div>
  )
}
