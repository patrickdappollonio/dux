import type { LucideIcon } from "lucide-react"
import { X } from "lucide-react"
import { type ComponentProps, type ReactNode, type Ref, useId } from "react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

// A shape, a tone, an optional icon, words and optional buttons. The caller
// chooses only those and the few named choices below; the border and fill
// strength, the spacing, the touch floor on the buttons and what a screen
// reader is told all belong to this component, so two notices cannot drift
// apart by accident. There is no prop for a class or a style.
//
// Shapes: a strip hangs across a pane (editor, Changes pane), a banner hangs
// across the whole app above its header, a box sits inside a dialog or card.
// Tones: warning, danger and info are the three meanings; neutral is the plain
// card surface for a notice that is a note about the app's own state.
//
// What a notice means to a screen reader is one named choice, `announce`:
// "alert" interrupts, "status" is read politely when it appears, "static" is a
// note that is there when the surface renders and is not read out. Left unset a
// danger notice is an alert and any other is a status.

export type NoticeShape = "strip" | "banner" | "box"
export type NoticeTone = "warning" | "danger" | "info" | "neutral"
export type NoticeAnnounce = "alert" | "status" | "static"
// "strong" is for the notice that is the app's loudest word of its kind (the
// open-to-everyone banner, the question that must be answered).
export type NoticeEmphasis = "normal" | "strong"
// The one place a notice is asked to hang: flush under a pane card's header.
export type NoticePlacement = "under-card-header"

interface ToneLook {
  frame: string
  strongFrame?: string
  icon: string
  // The colour of a title or lead sentence.
  heading: string
}

const TONE: Record<NoticeTone, ToneLook> = {
  warning: {
    frame: "border-dux-warning/40 bg-dux-warning/10",
    strongFrame: "border-dux-warning bg-dux-warning/10",
    icon: "text-dux-warning",
    heading: "text-dux-warning",
  },
  danger: {
    frame: "border-destructive/50 bg-destructive/10",
    strongFrame: "border-destructive/60 bg-destructive/20 text-foreground",
    icon: "text-destructive",
    heading: "text-destructive",
  },
  info: { frame: "border-border bg-muted/40 text-muted-foreground", icon: "", heading: "" },
  neutral: { frame: "border-border bg-card text-card-foreground", icon: "", heading: "" },
}

const DEFAULT_ANNOUNCE: Record<NoticeTone, "alert" | "status"> = {
  warning: "status",
  danger: "alert",
  info: "status",
  neutral: "status",
}

const ROLE: Record<NoticeAnnounce, "alert" | "status" | "note"> = {
  alert: "alert",
  status: "status",
  static: "note",
}

const PLACEMENT: Record<NoticePlacement, string> = { "under-card-header": "-mt-4" }

// The one button height rule: the 40px floor on a coarse pointer, and on a
// narrow screen too inside a pane or dialog. A banner's neighbours are only its
// own words and a gap of 2, so with a mouse the default height stands.
const TOUCH_FLOOR = "max-md:min-h-10 pointer-coarse:min-h-10"
const BANNER_TOUCH_FLOOR = "pointer-coarse:min-h-10"

export interface NoticeAction {
  label: string
  onSelect: () => void
  // "quiet" is the dismiss next to a main answer; "danger" is the answer that
  // throws the user's text away.
  look?: "default" | "quiet" | "danger"
  icon?: LucideIcon
  disabled?: boolean
  // Focus on mount, for the notice whose safe answer should have it.
  autoFocus?: boolean
  buttonRef?: Ref<HTMLButtonElement>
  testId?: string
}

const LOOK = { default: "outline", quiet: "ghost", danger: "destructive" } as const

export interface NoticeProps
  extends Omit<ComponentProps<"div">, "role" | "title" | "children" | "className" | "style"> {
  shape: NoticeShape
  tone: NoticeTone
  announce?: NoticeAnnounce
  emphasis?: NoticeEmphasis
  icon?: LucideIcon
  // The icon in another tone's colour, for a neutral card whose icon is the
  // only loud part.
  iconTone?: NoticeTone
  // A bold first line over the words, for a notice that says what happened and
  // then what it means.
  title?: ReactNode
  // A bold first sentence run into the words, in the tone's heading colour.
  lead?: ReactNode
  children: ReactNode
  // A row the notice holds under its words, full width (a checkbox that goes
  // with the warning).
  below?: ReactNode
  actions?: readonly NoticeAction[]
  // A close control on a strip's trailing end, for a notice that can be put away.
  onDismiss?: () => void
  placement?: NoticePlacement
}

export function Notice({
  shape,
  tone,
  announce,
  emphasis = "normal",
  icon: Icon,
  iconTone,
  title,
  lead,
  children,
  below,
  actions,
  onDismiss,
  placement,
  ...rest
}: NoticeProps) {
  const titleId = useId()
  const strip = shape === "strip"
  const banner = shape === "banner"
  const stacked = (!strip && !banner) || title != null
  const look = TONE[tone]
  const frame = emphasis === "strong" ? (look.strongFrame ?? look.frame) : look.frame
  const buttons = actions?.map((a) => (
    <Button
      key={a.label}
      ref={a.buttonRef}
      size={strip || banner ? "sm" : "default"}
      variant={LOOK[a.look ?? "default"]}
      disabled={a.disabled}
      autoFocus={a.autoFocus}
      data-testid={a.testId}
      onClick={a.onSelect}
      className={banner ? BANNER_TOUCH_FLOOR : TOUCH_FLOOR}
    >
      {a.icon ? <a.icon /> : null}
      {a.label}
    </Button>
  ))
  const words = (
    <>
      {lead != null ? <strong className={cn("font-semibold", look.heading)}>{lead}</strong> : null}
      {lead != null ? " " : null}
      {children}
    </>
  )
  const body = (
    // basis-40 lets a strip's buttons drop under the words on a narrow screen
    // instead of squeezing them to a sliver.
    <div className={cn("min-w-0 flex-1", strip && "basis-40")}>
      {title != null ? (
        <>
          <div id={titleId} className={cn("font-semibold", look.heading)}>
            {title}
          </div>
          <div className="mt-0.5 text-foreground/80">{words}</div>
        </>
      ) : (
        words
      )}
    </div>
  )
  const iconColor = TONE[iconTone ?? tone].icon
  const icon = Icon ? (
    <Icon
      aria-hidden
      className={cn(
        "shrink-0",
        // A banner's icon is larger and left off a phone, where the words need
        // the width.
        banner ? "hidden size-5 md:block" : "size-4",
        stacked && "mt-0.5",
        iconColor,
      )}
    />
  ) : null
  return (
    <div
      {...rest}
      role={ROLE[announce ?? DEFAULT_ANNOUNCE[tone]]}
      aria-labelledby={title != null ? titleId : undefined}
      data-toast-clear={strip ? "" : undefined}
      className={cn(
        "shrink-0 text-sm",
        frame,
        strip
          ? cn(
              "flex flex-wrap gap-x-2 gap-y-2 border-b px-3 py-2",
              stacked ? "items-start" : "items-center",
            )
          : banner
            ? "flex flex-col gap-2 border-b px-4 py-2.5 md:flex-row md:items-center md:gap-4"
            : "flex flex-col gap-3 rounded-md border p-3",
        placement && PLACEMENT[placement],
      )}
    >
      {strip || banner ? (
        <>
          {icon}
          {body}
          {buttons ? <div className="flex shrink-0 gap-2">{buttons}</div> : null}
          {onDismiss ? (
            // Ghost and icon-only: the sentence is its only neighbour, so a
            // stray tap lands on text; a finger still gets the 40px floor.
            <Button
              size="icon"
              variant="ghost"
              className="size-7 shrink-0 pointer-coarse:size-10"
              aria-label="Dismiss this notice"
              onClick={onDismiss}
            >
              <X />
            </Button>
          ) : null}
        </>
      ) : (
        <>
          <div className="flex items-start gap-2">
            {icon}
            {body}
          </div>
          {below}
          {buttons ? <div className="flex justify-end gap-3">{buttons}</div> : null}
        </>
      )}
    </div>
  )
}
