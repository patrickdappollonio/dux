// Which shell this device gets, the phone's or the computer's, answered in one
// place. `useIsMobile`, `isMobileViewport` and the `md:`/`max-md:` classes (by
// way of the `data-layout` attribute this module keeps on <html>) all read it,
// so the components and the stylesheet never disagree about the layout.
//
// The automatic answer: a window narrower than the breakpoint is a phone, and
// so is a touch device whose screen is short, sideways included, because a
// landscape phone is wide enough to pass for a computer and too short to use
// one's layout (the user's decision of 2026-11-21). A person's explicit choice
// on this device wins over both. That choice is per-device view state: kept in
// this browser's storage, never in config and never on the server.

/** Narrower than this is the phone layout, whatever the pointer. */
export const MOBILE_BREAKPOINT = 768

/**
 * A touch screen whose short side is below this many CSS px is a phone's.
 *
 * Phones measure 320 to 430 on their short side (the iPhone Pro Max is 430),
 * and the smallest tablets 600 (7-inch Android) to 744 (iPad mini), so 500
 * sits clear of both. It is the SCREEN's short side rather than the
 * viewport's height because the viewport loses height to the browser's
 * chrome and to the soft keyboard: a tablet typing with the keyboard up has a
 * viewport around 400 tall, and must not flip to the phone layout mid-message.
 * The screen's short side also does not change with rotation, which iOS
 * Safari's portrait-only `screen` dimensions require.
 */
export const SHORT_SCREEN_MAX = 500

/** The `localStorage` key for the person's choice on this device. */
export const LAYOUT_CHOICE_KEY = "dux:layout"

export type ShellLayout = "phone" | "computer"

export interface ShellLayoutInputs {
  /** The layout viewport's width in CSS px. */
  width: number
  /** The screen's shorter side in CSS px; 0 when the browser does not say. */
  screenShortSide: number
  /** Whether touch is the primary pointer. */
  coarse: boolean
  /** The person's choice on this device, or null to choose automatically. */
  choice: ShellLayout | null
}

export function resolveShellLayout(inputs: ShellLayoutInputs): ShellLayout {
  if (inputs.choice !== null) return inputs.choice
  if (inputs.width < MOBILE_BREAKPOINT) return "phone"
  // An unknown screen size (0) is not a short one.
  const shortScreen =
    inputs.screenShortSide > 0 && inputs.screenShortSide < SHORT_SCREEN_MAX
  return inputs.coarse && shortScreen ? "phone" : "computer"
}

const COARSE_POINTER_QUERY = "(pointer: coarse)"

function coarsePointer(): boolean {
  if (typeof window.matchMedia !== "function") return false
  return window.matchMedia(COARSE_POINTER_QUERY).matches
}

function screenShortSide(): number {
  const s = typeof screen === "undefined" ? undefined : screen
  if (!s) return 0
  return Math.min(s.width || 0, s.height || 0)
}

// Storage can be missing or throw (Safari private mode). Then the choice lives
// in memory for the life of the page; otherwise storage alone is read, so a
// choice another tab cleared is cleared here too.
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

let fallback: ShellLayout | null = null
let memoryOnly = false

/** The person's choice on this device; only a value this module wrote counts. */
export function readLayoutChoice(): ShellLayout | null {
  if (memoryOnly) return fallback
  let raw: string | null
  try {
    const store = storage()
    if (!store) return fallback
    raw = store.getItem(LAYOUT_CHOICE_KEY)
  } catch {
    return fallback
  }
  return raw === "phone" || raw === "computer" ? raw : null
}

interface Answer {
  layout: ShellLayout
  choice: ShellLayout | null
  phoneDevice: boolean
  narrowComputer: boolean
}

function computeAnswer(): Answer {
  const choice = readLayoutChoice()
  const device = {
    width: window.innerWidth,
    screenShortSide: screenShortSide(),
    coarse: coarsePointer(),
  }
  const layout = resolveShellLayout({ ...device, choice })
  return {
    layout,
    choice,
    phoneDevice: isPhoneHardware(device),
    narrowComputer: layout === "computer" && device.width < MOBILE_BREAKPOINT,
  }
}

// A touch screen whose short side is a phone's. The window's width is asked
// only when the screen does not say, because a tablet's window can be as narrow
// as a phone's (an iPad in Split View) while it can still drag a file in.
function isPhoneHardware(device: Omit<ShellLayoutInputs, "choice">): boolean {
  if (!device.coarse) return false
  if (device.screenShortSide > 0) return device.screenShortSide < SHORT_SCREEN_MAX
  return device.width < MOBILE_BREAKPOINT
}

// While anything subscribes, one set of window listeners keeps `cached` up to
// date and every read is answered from it; the soft keyboard fires `resize`
// over and over, and dozens of components ask. With nobody subscribed there is
// nothing to keep a cache fresh, so a read computes.
const listeners = new Set<() => void>()
let cached: Answer | null = null
let detach: (() => void) | null = null

function refresh(): void {
  if (cached === null) return
  const next = computeAnswer()
  if (
    next.layout === cached.layout &&
    next.choice === cached.choice &&
    next.phoneDevice === cached.phoneDevice &&
    next.narrowComputer === cached.narrowComputer
  ) {
    return
  }
  cached = next
  for (const listener of [...listeners]) listener()
}

function attach(): () => void {
  const onStorage = (event: StorageEvent) => {
    if (event.key === LAYOUT_CHOICE_KEY || event.key === null) refresh()
  }
  const mql =
    typeof window.matchMedia === "function"
      ? window.matchMedia(COARSE_POINTER_QUERY)
      : null
  window.addEventListener("resize", refresh)
  window.addEventListener("storage", onStorage)
  mql?.addEventListener("change", refresh)
  return () => {
    window.removeEventListener("resize", refresh)
    window.removeEventListener("storage", onStorage)
    mql?.removeEventListener("change", refresh)
  }
}

function answer(): Answer {
  return cached ?? computeAnswer()
}

/** Store the choice, or `null` to choose automatically again. */
export function setLayoutChoice(next: ShellLayout | null): void {
  fallback = next
  try {
    const store = storage()
    if (store) {
      if (next === null) store.removeItem(LAYOUT_CHOICE_KEY)
      else store.setItem(LAYOUT_CHOICE_KEY, next)
      memoryOnly = false
    }
  } catch {
    memoryOnly = true
  } finally {
    refresh()
  }
}

/** The layout this device shows right now. Outside a browser: the computer's. */
export function currentShellLayout(): ShellLayout {
  if (typeof window === "undefined") return "computer"
  return answer().layout
}

/** The person's choice on this device as the layout last read it. */
export function currentLayoutChoice(): ShellLayout | null {
  if (typeof window === "undefined") return null
  return answer().choice
}

/**
 * Whether this device is a touch screen whose short side is a phone's, whatever
 * layout a person chose and however narrow its window: the question for what
 * the hardware can do (drag a file in) rather than for how the page is laid out.
 */
export function isPhoneDevice(): boolean {
  if (typeof window === "undefined") return false
  return answer().phoneDevice
}

/**
 * Whether the computer layout is shown in a window narrower than the
 * breakpoint, which only a person's choice brings about: too narrow for the
 * expanded sidebar beside the page.
 */
export function isNarrowComputerLayout(): boolean {
  if (typeof window === "undefined") return false
  return answer().narrowComputer
}

/**
 * Subscribe to everything that can move the answer: the choice, the window's
 * size (rotation included), the primary pointer, and a choice another tab of
 * this browser stored. The listener runs only when an answer changed. Returns the unsubscribe.
 */
export function subscribeShellLayout(listener: () => void): () => void {
  if (listeners.size === 0) {
    cached = computeAnswer()
    detach = attach()
  }
  listeners.add(listener)
  return () => {
    if (!listeners.delete(listener) || listeners.size > 0) return
    detach?.()
    detach = null
    cached = null
  }
}

/**
 * Keep `data-layout` on <html> equal to the current answer; the stylesheet's
 * `md:` and `max-md:` variants key on it (see index.css). Called once before
 * the first render, so no frame is styled for the wrong layout.
 */
export function installShellLayoutAttribute(doc: Document): () => void {
  const apply = () => {
    const next = currentShellLayout()
    if (doc.documentElement.dataset.layout !== next) {
      doc.documentElement.dataset.layout = next
    }
  }
  apply()
  return subscribeShellLayout(apply)
}
