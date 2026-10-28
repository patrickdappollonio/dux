import { useEffect, useLayoutEffect, useRef } from "react"
import { Toaster as Sonner, type ToasterProps } from "sonner"

import { useIsMobile } from "@/hooks/use-mobile"
import { CircleCheckIcon, InfoIcon, TriangleAlertIcon, OctagonXIcon, Loader2Icon } from "lucide-react"

// Which drags dismiss a toast. Named explicitly because sonner infers its defaults
// by splitting the position string, and "center" is not a direction, so a sideways
// swipe would do nothing. Only the direction that pushes a toast off its own edge
// is listed. A busy toast has neither exit: sonner draws it no close button and
// ignores a swipe on it, so `lib/notify.ts` retires it.
export const TOAST_SWIPE_DIRECTIONS: ToasterProps["swipeDirections"] = [
  "bottom",
  "left",
  "right",
]

/// The same rule read off the top edge, for the phone's placement below: the
/// vertical direction follows the anchor, so a top toast is pushed up and off.
export const TOAST_SWIPE_DIRECTIONS_TOP: ToasterProps["swipeDirections"] = [
  "top",
  "left",
  "right",
]

/// Where the toasts sit, per shell: the phone anchors at the TOP always, because
/// the bottom of a pane screen is its typing surface, and the computer keeps the
/// bottom. Both offsets carry the same expression, because sonner switches to its
/// mobile variables at its own 600px query rather than the app's 768px breakpoint,
/// and the safe-area inset is added here, since the toaster is fixed and sits
/// outside the mobile root that pads for the notch.
const TOP_INSET = "calc(env(safe-area-inset-top) + 1rem)"
const BOTTOM_INSET = "calc(env(safe-area-inset-bottom) + 2.5rem)"

export function toastPlacement(isMobile: boolean): {
  position: NonNullable<ToasterProps["position"]>
  offset: NonNullable<ToasterProps["offset"]>
  mobileOffset?: ToasterProps["mobileOffset"]
  swipeDirections: ToasterProps["swipeDirections"]
} {
  if (isMobile) {
    return {
      position: "top-center",
      offset: { top: TOP_INSET },
      mobileOffset: { top: TOP_INSET },
      swipeDirections: TOAST_SWIPE_DIRECTIONS_TOP,
    }
  }
  // No `mobileOffset` on this branch: sonner's mobile variables are unreachable
  // from a viewport wide enough to be on this shell.
  return {
    position: "bottom-center",
    offset: { bottom: BOTTOM_INSET },
    swipeDirections: TOAST_SWIPE_DIRECTIONS,
  }
}

// Per-tone color, worn by the icon and by the countdown bar so a toast's two
// marks always agree. The icon adds it to a shape that already differs per tone,
// so color is never the only carrier.
const TONE_COLOR = {
  success: "text-dux-success",
  info: "text-dux-info",
  warning: "text-dux-warning",
  error: "text-destructive",
} as const

// The loading spinner stays muted: in progress is not a severity.
const TONE_ICON = {
  success: `size-4 ${TONE_COLOR.success}`,
  info: `size-4 ${TONE_COLOR.info}`,
  warning: `size-4 ${TONE_COLOR.warning}`,
  error: `size-4 ${TONE_COLOR.error}`,
  loading: "size-4 animate-spin text-muted-foreground",
} as const

/// The foot of every toast `lib/notify.ts` raises: a bar draining over `ms`, the
/// toast's exact sonner duration, and nothing at all when the toast will not
/// dismiss itself (`ms` null). The bar stopping, dimmed while the stack is open,
/// is the only sign of a pause. It sits in the toast's own bottom padding,
/// absolutely placed, so it takes no room in it and covers nothing outside it,
/// and it is hidden from the toaster's live region.
///
/// The bar keeps no clock of its own. It is a CSS animation that `index.css`
/// pauses on exactly the conditions sonner pauses on (see `Toaster` below), and
/// a raise that replaces a toast hands a fresh element key, so the bar starts
/// over with sonner's timer.
///
/// sonner (re)arms its timer in an effect of the very commit that mounts the
/// toast or releases the stack, while a CSS animation that starts or resumes
/// waits for the frame that paints it (measured 130 to 300ms late on a busy
/// page). So whenever the bar goes from waiting to running, at mount or on a
/// release, its start is anchored to that moment instead, keeping what it had
/// already drained.
export function ToastCountdown({ tone, ms }: { tone: string; ms: number | null }) {
  // An unknown tone is raised as a success, so it drains in the success color.
  const color = TONE_COLOR[tone as keyof typeof TONE_COLOR] ?? TONE_COLOR.success
  const fillRef = useRef<HTMLSpanElement>(null)
  useLayoutEffect(() => {
    const fill = fillRef.current
    if (!fill || typeof fill.getAnimations !== "function") return
    const anchor = () => {
      // Reading the animations flushes style, so a release has already turned
      // a held bar into one pending to run.
      for (const animation of fill.getAnimations()) {
        if (animation.playState !== "running" || !animation.pending) continue
        const drained = Number(animation.currentTime ?? 0)
        animation.startTime = performance.now() - drained
      }
    }
    anchor()
    // The hold conditions live on the toast (sonner's `data-expanded`) and on
    // the toaster's section (the mirrored press and hidden flags).
    const observer = new MutationObserver(anchor)
    const toast = fill.closest("[data-sonner-toast]")
    const section = fill.closest("section")
    if (toast) observer.observe(toast, { attributeFilter: ["data-expanded"] })
    if (section) {
      observer.observe(section, { attributeFilter: ["data-dux-pressed", "data-dux-hidden"] })
    }
    return () => observer.disconnect()
  }, [])
  if (ms === null) return null
  return (
    <span aria-hidden className="dux-toast-drain">
      <span
        ref={fillRef}
        className={`dux-toast-drain-fill bg-current ${color}`}
        style={
          {
            "--dux-drain-ms": `${ms}ms`,
            "--dux-drain-steps": Math.max(1, Math.round(ms / 1000)),
          } as React.CSSProperties
        }
      />
    </span>
  )
}

/// How many toasts stack before the rest queue behind them. Above sonner's default
/// of 3, which a multi-step operation's keyed statuses fill on their own.
export const VISIBLE_TOASTS_DESKTOP = 5

/// Phones keep sonner's 3. Vertical space is scarce there and the toasts sit
/// over the terminal, which is the thing the user is reading.
export const VISIBLE_TOASTS_MOBILE = 3

/// How tall a toast's text may grow before it scrolls inside the toast. A phone
/// lets the WHOLE toast reach 75% of the visible screen (`dvh`, so the soft
/// keyboard and the browser's own bars are taken off): the text gets that less
/// the toast's own 16px padding and 1px border on each side, since the icon and
/// any button sit beside the text rather than above it. A computer keeps the
/// short cap, where a wheel scrolls the rest.
function toastTextCap(isMobile: boolean): string {
  return isMobile ? "max-h-[calc(75dvh-34px)]" : "max-h-[min(12rem,40vh)]"
}

const Toaster = ({ ...props }: ToasterProps) => {
  const isMobile = useIsMobile()
  // Read live, so a rotation across the shell breakpoint moves the stack with the
  // shell. Crossing it re-keys sonner's per-position list, so open toasts remount
  // and their dismissal timers restart, which is the accepted cost.
  const placement = toastPlacement(isMobile)
  // sonner pauses every toast's timer while the stack is expanded (published as
  // `data-expanded` on each toast), while a pointer is down on the stack, and
  // while the page is hidden. The last two are React state it does not publish,
  // so they are mirrored here off the same events it reads, and the countdown
  // bar pauses on exactly what sonner pauses on.
  //
  // A toast's text is selectable, so more rules live here, on the stack as a
  // whole rather than per toast:
  //
  // - Only a finger swipes. A mouse or pen drag is a selection. Decided per
  //   press from its own pointer type rather than from a media query, so a
  //   touchscreen laptop's finger still swipes while its mouse selects; the
  //   close button stays the mouse's exit. sonner's swipe handler is reached
  //   through React's listener at the app root, so nothing between the toast
  //   and that root can withhold a move from sonner without withholding it
  //   from every listener above the root too (a sidebar row dragged across a
  //   toast would stall). So the moves travel on, and what sonner makes of them
  //   is undone: the swipe offset it writes on the toast is zeroed once the
  //   move has passed the root, again before sonner reads it on the release so
  //   a selecting drag never dismisses, and the stack's `data-dux-selecting`
  //   keeps the text selectable through sonner's swiped state (`index.css`).
  // - A release that leaves a selection inside the stack is held back from
  //   sonner, whose timers stay paused as if the press were still down, and the
  //   held release is handed over once the selection leaves the stack (its
  //   toast removed included, which need not raise a selectionchange) or the
  //   next press lands anywhere. Anywhere, because a press on the page's
  //   unselectable chrome leaves the selection where it was, and the toast must
  //   not stay up for good on a highlight nobody is copying any more. A press
  //   on the stack itself simply supersedes it, since sonner sees that press
  //   and its release. Without the hold, a selection whose drag ended off the
  //   toast would dismiss with the countdown while it is being copied. Only a
  //   press sonner saw is held: a release over a toast for a press that began
  //   elsewhere passes untouched.
  // - A press that ends with no release sonner can hear (cancelled, turned into
  //   a native drag of the selected text, or released off the stack after its
  //   toast was removed and took the pointer capture with it) is ended here,
  //   since sonner listens for the release alone and would otherwise hold every
  //   timer for good. One that leaves the selection in the stack is held like
  //   any other.
  // - A release sent here is a real `pointerup` with the original press's
  //   pointer, because sonner's handler is reached only through React's
  //   listener at the app root, so it passes every listener above the root on
  //   the way. It is therefore sent in a task of its own and only while no
  //   pointer is down anywhere on the page, so it can never land inside
  //   somebody else's press (a divider drag that tracks presses on the window).
  // - sonner ignores the pointer leaving the stack while a press is down, and
  //   nothing after the release tells it again, so a drag that ended off the
  //   stack would leave it expanded and every timer paused until the pointer
  //   next passed over it. Once a mouse or pen release has reached sonner with
  //   the pointer elsewhere, the stack is told the pointer left. It waits a
  //   task so sonner's handler reads the released state rather than the
  //   pressed one. A finger is left to sonner, whose tap expands the stack.
  const sectionRef = useRef<HTMLElement>(null)
  useEffect(() => {
    const section = sectionRef.current
    if (!section) return
    const lists = () => section.querySelectorAll("[data-sonner-toaster]")
    type Press = { toast: HTMLElement | null; pointerType: string; pointerId: number }
    // The press on the stack in progress, from its pointerdown until it ends.
    let down: Press | null = null
    // A release held back while a selection lies in the stack.
    let held: Press | null = null
    const zeroSwipe = (toast: HTMLElement | null) => {
      toast?.style.setProperty("--swipe-amount-x", "0px")
      toast?.style.setProperty("--swipe-amount-y", "0px")
    }
    // sonner's own list handler: any press counts, except one landing on a toast
    // raised as not dismissible.
    const press = (e: PointerEvent) => {
      const target = e.target instanceof HTMLElement ? e.target : null
      down = {
        toast: target?.closest<HTMLElement>("[data-sonner-toast]") ?? null,
        pointerType: e.pointerType,
        pointerId: e.pointerId,
      }
      section.toggleAttribute("data-dux-selecting", e.pointerType !== "touch")
      if (target?.dataset.dismissible === "false") return
      section.setAttribute("data-dux-pressed", "")
    }
    const moved = (e: PointerEvent) => {
      if (down && down.pointerType !== "touch" && e.pointerType !== "touch") zeroSwipe(down.toast)
    }
    const selectionInside = () => {
      const selection = document.getSelection()
      if (!selection || selection.isCollapsed) return false
      return section.contains(selection.anchorNode) || section.contains(selection.focusNode)
    }

    // Every press on the page still down, so a release sent here waits for
    // them all to end.
    const pointersDown = new Set<number>()
    const replayed = new WeakSet<Event>()
    let queued: Press[] = []
    let sendTimer: ReturnType<typeof setTimeout> | undefined
    // A mouse or pen release goes to the toast it started on, so that toast's
    // handler sees the press end too (its swipe offset is zero, so it dismisses
    // nothing). A finger's goes to the lists alone, whose handler holds the
    // timers, because the offset a finger left behind would otherwise be judged
    // as a swipe. So does any release whose toast has gone.
    const deliver = ({ toast, pointerType, pointerId }: Press) => {
      const init = { bubbles: true, cancelable: true, pointerType, pointerId }
      const targets = pointerType !== "touch" && toast?.isConnected ? [toast] : [...lists()]
      for (const to of targets) {
        const up = new PointerEvent("pointerup", init)
        replayed.add(up)
        to.dispatchEvent(up)
      }
    }
    const sendWhenFree = () => {
      clearTimeout(sendTimer)
      if (queued.length === 0 || pointersDown.size > 0) return
      sendTimer = setTimeout(() => {
        if (pointersDown.size > 0) return
        const releases = queued
        queued = []
        releases.forEach(deliver)
      }, 0)
    }
    const send = (release: Press) => {
      queued.push(release)
      sendWhenFree()
    }
    const hold = (release: Press) => {
      held = release
      down = null
    }
    // The press ends without sonner hearing a release: held if the selection is
    // in the stack, otherwise a release is sent.
    const endUnheard = (release: Press) => {
      if (release.pointerType !== "touch") zeroSwipe(release.toast)
      if (selectionInside()) hold(release)
      else {
        down = null
        send(release)
      }
    }

    let leaveTimer: ReturnType<typeof setTimeout> | undefined
    const tellPointerLeft = () => {
      clearTimeout(leaveTimer)
      leaveTimer = setTimeout(() => {
        if (section.matches(":hover")) return
        for (const list of lists()) {
          list.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }))
        }
      }, 0)
    }
    const ended = (pointerType: string) => {
      section.removeAttribute("data-dux-selecting")
      section.removeAttribute("data-dux-pressed")
      if (pointerType !== "touch") tellPointerLeft()
    }
    const release = (e: PointerEvent) => {
      if (replayed.has(e)) {
        ended(e.pointerType)
        return
      }
      const pressed = down
      if (!pressed || pressed.pointerId !== e.pointerId) return
      if (pressed.pointerType !== "touch") zeroSwipe(pressed.toast)
      if (selectionInside()) {
        e.stopPropagation()
        // `data-dux-selecting` stays until the hand-over: sonner's toast still
        // counts itself swiped, and the selection must stay copyable.
        hold({ ...pressed, toast: e.target instanceof HTMLElement ? e.target : pressed.toast })
        return
      }
      down = null
      ended(e.pointerType)
    }
    const handOver = () => {
      const release = held
      if (!release || selectionInside()) return
      held = null
      send(release)
    }

    const pagePressed = (e: PointerEvent) => {
      pointersDown.add(e.pointerId)
      clearTimeout(sendTimer)
      if (!held) return
      if (e.target instanceof Node && section.contains(e.target)) held = null
      else {
        send(held)
        held = null
      }
    }
    const pageReleased = (e: PointerEvent) => {
      if (replayed.has(e)) return
      pointersDown.delete(e.pointerId)
      // The stack's own listener ends a press released on it; this one ends a
      // press whose release landed elsewhere, or that was cancelled.
      const pressed = down
      if (pressed && pressed.pointerId === e.pointerId) {
        const onStack = e.target instanceof Node && section.contains(e.target)
        if (e.type === "pointercancel" || !onStack) endUnheard(pressed)
      }
      sendWhenFree()
    }
    const dragStarted = () => {
      if (down) endUnheard(down)
    }
    const dragEnded = () => {
      // A native drag swallows the release of the press that began it.
      pointersDown.clear()
      sendWhenFree()
    }
    const removals = new MutationObserver(handOver)
    removals.observe(section, { childList: true, subtree: true })
    const visibility = () => section.toggleAttribute("data-dux-hidden", document.hidden)
    visibility()
    window.addEventListener("pointerdown", pagePressed, true)
    window.addEventListener("pointerup", pageReleased, true)
    window.addEventListener("pointercancel", pageReleased, true)
    window.addEventListener("dragend", dragEnded, true)
    section.addEventListener("pointerdown", press)
    document.addEventListener("pointermove", moved)
    section.addEventListener("pointerup", release, true)
    document.addEventListener("dragstart", dragStarted, true)
    document.addEventListener("selectionchange", handOver)
    document.addEventListener("visibilitychange", visibility)
    return () => {
      clearTimeout(leaveTimer)
      clearTimeout(sendTimer)
      removals.disconnect()
      window.removeEventListener("pointerdown", pagePressed, true)
      window.removeEventListener("pointerup", pageReleased, true)
      window.removeEventListener("pointercancel", pageReleased, true)
      window.removeEventListener("dragend", dragEnded, true)
      section.removeEventListener("pointerdown", press)
      document.removeEventListener("pointermove", moved)
      section.removeEventListener("pointerup", release, true)
      document.removeEventListener("dragstart", dragStarted, true)
      document.removeEventListener("selectionchange", handOver)
      document.removeEventListener("visibilitychange", visibility)
    }
  }, [])
  return (
    <Sonner
      ref={sectionRef}
      theme="dark"
      visibleToasts={isMobile ? VISIBLE_TOASTS_MOBILE : VISIBLE_TOASTS_DESKTOP}
      className="toaster group"
      {...placement}
      // Every toast auto-dismisses on a severity-graded timer (`lib/notify.ts`), so
      // the close button is a mouse shortcut rather than the only exit.
      closeButton
      icons={{
        success: (
          <CircleCheckIcon className={TONE_ICON.success} />
        ),
        info: (
          <InfoIcon className={TONE_ICON.info} />
        ),
        warning: (
          <TriangleAlertIcon className={TONE_ICON.warning} />
        ),
        error: (
          <OctagonXIcon className={TONE_ICON.error} />
        ),
        loading: (
          <Loader2Icon className={TONE_ICON.loading} />
        ),
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      toastOptions={{
        classNames: {
          // `allow-select` lifts the page's blanket `user-select: none`, so a
          // toast's text, name chips included, can be selected and copied
          // before it dismisses itself.
          toast: "cn-toast allow-select",
          // Producers do not know how long their text runs (a list of skipped
          // paths once ran to 2,500 characters), so the text caps its own
          // height and scrolls inside the toast rather than covering the
          // screen. `overscroll-contain` keeps a wheel at the end of the text
          // from scrolling the page underneath. `touch-none` is load-bearing:
          // a scroll container hands every finger pan back to the browser
          // (even with nothing to scroll, sonner's own `touch-action: none` on
          // the toast notwithstanding), which cancels the pointer stream and
          // left a touch swipe on the text unable to dismiss the toast. The
          // cost is that a finger cannot scroll an overflowing text, so the
          // phone's cap is tall enough that a long toast rarely needs to.
          // `overflow-x-hidden` keeps the text from scrolling sideways: once
          // one axis scrolls the other computes to auto, and a wrapped chip's
          // end padding pokes a few pixels past the column.
          title: `${toastTextCap(isMobile)} overflow-x-hidden overflow-y-auto overscroll-contain touch-none`,
          // The description slot carries only `ToastCountdown`, whose parts are
          // absolutely placed against the toast: `contents` keeps the slot from
          // adding a box, and the content column's gap with it, under the text.
          description: "contents",
        },
      }}
      {...props}
    />
  )
}

export { Toaster }
