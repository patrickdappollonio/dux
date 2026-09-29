import * as React from "react"

/** The event props the animated element must carry so the hook can see its
 *  animation start and, where the Web Animations API is missing, reach its
 *  cycle boundaries. */
export type SettlingAnimationHandlers = {
  onAnimationStart: (event: React.AnimationEvent) => void
  onAnimationIteration: (event: React.AnimationEvent) => void
  onAnimationEnd: (event: React.AnimationEvent) => void
}

function pageHidden(): boolean {
  return typeof document !== "undefined" && document.hidden
}

/**
 * Keeps a looping CSS animation applied after `active` turns false until the
 * iteration in flight finishes, so a stop comes to rest exactly on the
 * animation's resting keyframe instead of freezing, snapping or gliding back
 * from mid-cycle.
 *
 * `running` is what the caller keys the animation class on, and `attach` (a
 * callback ref) and `handlers` go on the element that carries it. On a stop the hook finds the
 * running animation through the Web Animations API and shortens it to end at
 * the end of its current iteration, then lets the class go when the browser
 * reports it finished: the last frame painted is the keyframes' 100%, with no
 * event handled a task later landing a frame into the next cycle. Work resuming
 * first restores infinite iterations on the same animation, so nothing
 * restarts. Where `getAnimations` is missing, the next `animationiteration` (or
 * `animationend`) of `animationName` is the fallback signal.
 *
 * A stop is immediate when there is nothing to finish: the animation never
 * reported `animationstart` (reduced motion gates it off in CSS, or the
 * element is not rendered), the browser cancelled it, the tab is hidden and
 * nothing is painted, or `cancel` says a different state has taken the element
 * over.
 */
export function useSettlingAnimation<T extends Element>(
  active: boolean,
  animationName: string,
  { cancel = false }: { cancel?: boolean } = {},
): {
  running: boolean
  attach: (el: T | null) => void
  handlers: SettlingAnimationHandlers
} {
  // A callback ref kept in state, so the effects below re-run for a new element.
  const [el, setEl] = React.useState<T | null>(null)
  // Whether the browser has actually started the animation this run.
  const [started, setStarted] = React.useState(false)
  const [settling, setSettling] = React.useState(false)
  const [prevActive, setPrevActive] = React.useState(active)

  // A state update made during render lands on the NEXT render, so this render
  // reads the value it is about to store rather than the stale one.
  let settlingNow = settling
  if (active !== prevActive) {
    setPrevActive(active)
    // Resuming mid-settle ends the settle without touching the running
    // animation; stopping settles only when there is a painted cycle to finish.
    settlingNow = !active && started && !pageHidden()
    setSettling(settlingNow)
  }
  const holding = settlingNow && !cancel
  const running = !cancel && (active || holding)
  if (!running && (started || settlingNow)) {
    // The class is gone, so the animation is too: forget this run.
    setStarted(false)
    setSettling(false)
  }

  // The browser cancels a CSS animation whose element stops rendering, or that
  // reduced motion switched off mid-run, and no boundary ever follows. React
  // has no prop for the event, so it is heard natively.
  React.useEffect(() => {
    if (!el) return
    const onCancel = (event: Event) => {
      if ((event as AnimationEvent).animationName !== animationName) return
      setStarted(false)
      setSettling(false)
    }
    el.addEventListener("animationcancel", onCancel)
    return () => el.removeEventListener("animationcancel", onCancel)
  }, [el, animationName])

  React.useLayoutEffect(() => {
    if (!holding) return
    let live = true
    const finish = () => {
      if (live) setSettling(false)
    }
    const onVisibility = () => {
      if (pageHidden()) finish()
    }
    document.addEventListener("visibilitychange", onVisibility)
    const cleanupVisibility = () =>
      document.removeEventListener("visibilitychange", onVisibility)

    if (!el || typeof el.getAnimations !== "function") {
      // No Web Animations API: the boundary event handler ends the settle.
      return () => {
        live = false
        cleanupVisibility()
      }
    }
    const animation = el
      .getAnimations()
      .find((a) => (a as CSSAnimation).animationName === animationName)
    const effect = animation?.effect
    if (!animation || !effect) {
      // It was running a moment ago and is gone now, so there is nothing left
      // to finish.
      finish()
      return cleanupVisibility
    }
    const { currentIteration } = effect.getComputedTiming()
    effect.updateTiming({ iterations: (currentIteration ?? 0) + 1 })
    // A cancel rejects the promise, and a cancelled animation is just as done.
    animation.finished.then(finish, finish)
    return () => {
      live = false
      cleanupVisibility()
      // Resuming keeps this same animation looping from where it is. On a stop
      // the class is leaving in this same commit, which cancels it anyway.
      effect.updateTiming({ iterations: Infinity })
    }
  }, [el, holding, animationName])

  const onAnimationStart = React.useCallback(
    (event: React.AnimationEvent) => {
      if (event.animationName === animationName) setStarted(true)
    },
    [animationName],
  )
  const onBoundary = React.useCallback(
    (event: React.AnimationEvent) => {
      if (event.animationName !== animationName) return
      // With the Web Animations API the finish owns the stop; a boundary event
      // is handled a task late and would cut the next iteration's first frame.
      if (typeof event.currentTarget.getAnimations === "function") return
      // A boundary while still active is a no-op: `settling` is already false.
      setSettling(false)
    },
    [animationName],
  )

  const handlers = React.useMemo(
    () => ({
      onAnimationStart,
      onAnimationIteration: onBoundary,
      onAnimationEnd: onBoundary,
    }),
    [onAnimationStart, onBoundary],
  )
  return { running, attach: setEl, handlers }
}
