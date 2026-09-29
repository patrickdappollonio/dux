import * as React from "react"

/** The event props the animated element must carry so the hook can see its
 *  animation start and reach each cycle boundary. */
export type SettlingAnimationHandlers = {
  onAnimationStart: (event: React.AnimationEvent) => void
  onAnimationIteration: (event: React.AnimationEvent) => void
  onAnimationEnd: (event: React.AnimationEvent) => void
}

/**
 * Keeps a looping CSS animation applied after `active` turns false until the
 * cycle in flight finishes, so a stop comes to rest exactly on the animation's
 * start frame instead of freezing, snapping or gliding back from mid-cycle.
 *
 * `running` is what the caller keys the animation class on. It stays true
 * while active, and after a stop until the next `animationiteration` (or
 * `animationend`) of `animationName` on the element carrying `handlers`. Work
 * resuming before that boundary keeps the same animation running, because
 * dropping and re-adding the class would restart it from frame zero.
 *
 * A stop is immediate when there is no cycle to finish: when the animation
 * never reported `animationstart` (reduced motion gates it off in CSS, the
 * element is not rendered, or the environment runs no CSS), and when `cancel`
 * says a different state has taken the element over. `timeoutMs` bounds the
 * wait for the case where the animation is cancelled out from under the settle
 * and no boundary will ever come.
 */
export function useSettlingAnimation(
  active: boolean,
  animationName: string,
  { cancel = false, timeoutMs = 5000 }: { cancel?: boolean; timeoutMs?: number } = {},
): { running: boolean; handlers: SettlingAnimationHandlers } {
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
    // animation; stopping settles only when there is a cycle to finish.
    settlingNow = !active && started
    setSettling(settlingNow)
  }
  const holding = settlingNow && !cancel
  const running = !cancel && (active || holding)
  if (!running && (started || settlingNow)) {
    // The class is gone, so the animation is too: forget this run.
    setStarted(false)
    setSettling(false)
  }

  React.useEffect(() => {
    if (!holding) return
    const timer = window.setTimeout(() => {
      setSettling(false)
      setStarted(false)
    }, timeoutMs)
    return () => window.clearTimeout(timer)
  }, [holding, timeoutMs])

  const onAnimationStart = React.useCallback(
    (event: React.AnimationEvent) => {
      if (event.animationName === animationName) setStarted(true)
    },
    [animationName],
  )
  const onBoundary = React.useCallback(
    (event: React.AnimationEvent) => {
      if (event.animationName !== animationName) return
      // A boundary while still active is just another cycle; the state update
      // is a no-op then, because `settling` is already false.
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
  return { running, handlers }
}
