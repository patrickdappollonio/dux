import { Loader2 } from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import {
  GLYPH_SPINNER_CLASS,
  SPINNER_FRAMES,
  SPINNER_FRAME_MS,
} from "@/lib/spinnerFrames"
import { cn } from "@/lib/utils"

// The web UI's one loading indicator, in two kinds:
//
// - `glyph`: the six stepping arcs the terminal UI and its status line spin
//   with, drawn as a character of the line it sits in (`spinnerFrames.ts` has
//   the frames and cadence, index.css the fixed-width slot and the face that
//   carries them). Its size is the line's text (`text`) or the large text
//   size (`lg`).
// - `ring`: a turning ring, sized as an icon (`xs` 12px, `sm` 14px, `md` 16px,
//   `lg` 20px), for a slot that holds an icon when nothing is loading.
//
// Every caller names its kind and its size, so neither is ever a default
// nobody chose. The colour is the surrounding text's unless `tone` asks for
// the muted or the primary one.
//
// Neither kind moves under reduced motion: the ring stands still and the glyph
// holds its first frame, so the page still says "loading" without motion.
// Both are decorative: whatever the spinner stands for is said in words beside
// it.

const TONES = {
  current: "",
  muted: "text-muted-foreground",
  primary: "text-primary",
} as const

export type SpinnerTone = keyof typeof TONES

const GLYPH_SIZES = {
  text: "",
  lg: "text-lg",
} as const

const RING_SIZES = {
  xs: "size-3",
  sm: "size-3.5",
  md: "size-4",
  lg: "size-5",
} as const

export type RingSize = keyof typeof RING_SIZES

export type SpinnerProps =
  | { kind: "glyph"; size: keyof typeof GLYPH_SIZES; tone?: SpinnerTone }
  | { kind: "ring"; size: RingSize; tone?: SpinnerTone }

export function Spinner(props: SpinnerProps) {
  const tone = TONES[props.tone ?? "current"]
  if (props.kind === "glyph") {
    return <GlyphFrames className={cn(GLYPH_SIZES[props.size], tone)} />
  }
  return (
    <Loader2
      aria-hidden="true"
      className={cn("shrink-0 motion-safe:animate-spin", RING_SIZES[props.size], tone)}
    />
  )
}

function GlyphFrames({ className }: { className: string }) {
  const reduceMotion = usePrefersReducedMotion()
  const [i, setI] = useState(0)
  useEffect(() => {
    if (reduceMotion) return
    const t = setInterval(
      () => setI((n) => (n + 1) % SPINNER_FRAMES.length),
      SPINNER_FRAME_MS,
    )
    return () => clearInterval(t)
  }, [reduceMotion])
  return (
    <span aria-hidden="true" className={cn(GLYPH_SPINNER_CLASS, className)}>
      {SPINNER_FRAMES[reduceMotion ? 0 : i]}
    </span>
  )
}

// A spinner and the sentence that says what is loading, on one muted line.
// The spinner's size follows the sentence's: the glyph is a character of it,
// and the ring is 16px beside `sm` text and 14px beside `xs`. Where the line
// sits (padding, centring) is the caller's. `announce` makes the sentence a
// live region, for a line that appears without the user asking for it.
export function LoadingSentence({
  spinner,
  size,
  announce = false,
  children,
}: {
  spinner: SpinnerProps["kind"]
  size: "sm" | "xs"
  announce?: boolean
  children: ReactNode
}) {
  return (
    <span
      className={cn(
        "flex items-center gap-2 text-muted-foreground",
        size === "sm" ? "text-sm" : "text-xs",
      )}
    >
      {spinner === "glyph" ? (
        <Spinner kind="glyph" size="text" />
      ) : (
        <Spinner kind="ring" size={size === "sm" ? "md" : "sm"} />
      )}
      <span role={announce ? "status" : undefined}>{children}</span>
    </span>
  )
}
