import type { LucideIcon } from "lucide-react"
import { X } from "lucide-react"
import { type ComponentProps, type ReactNode, type Ref, useId } from "react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

// A shape (a strip across a pane, or a box inside a dialog or card), a tone,
// an optional icon, words and optional buttons. The caller chooses only those;
// the border and fill strength, the spacing, the touch floor on the buttons and
// what a screen reader is told all belong to this component, so two notices
// cannot drift apart by accident.
//
// What is announced follows the tone: a danger notice is an alert, a warning or
// an info notice is a polite status. A caller never writes a role.
//
// A strip hangs from the top of a pane, so it carries `data-toast-clear` and
// the phone's toasts start below it.

export type NoticeShape = "strip" | "box"
export type NoticeTone = "warning" | "danger" | "info"

const TONE: Record<NoticeTone, { frame: string; icon: string }> = {
  warning: { frame: "border-dux-warning/40 bg-dux-warning/10", icon: "text-dux-warning" },
  danger: { frame: "border-destructive/50 bg-destructive/10", icon: "text-destructive" },
  info: { frame: "border-border bg-muted/40 text-muted-foreground", icon: "" },
}

const ROLE: Record<NoticeTone, "alert" | "status"> = {
  warning: "status",
  danger: "alert",
  info: "status",
}

// The one button height rule for every notice: the 40px floor on a narrow
// screen and on a coarse pointer. Neighbours are the notice's own text and a
// gap of 2 or 3 between buttons.
const TOUCH_FLOOR = "max-md:min-h-10 pointer-coarse:min-h-10"

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
  extends Omit<ComponentProps<"div">, "role" | "title" | "children" | "className"> {
  shape: NoticeShape
  tone: NoticeTone
  icon?: LucideIcon
  // A bold first line over the words, for a notice that says what happened and
  // then what it means.
  title?: ReactNode
  children: ReactNode
  actions?: readonly NoticeAction[]
  // A close control on a strip's trailing end, for a notice that can be put away.
  onDismiss?: () => void
  // Where the notice sits among its neighbours (a margin), never how it looks.
  placement?: string
}

export function Notice({
  shape,
  tone,
  icon: Icon,
  title,
  children,
  actions,
  onDismiss,
  placement,
  ...rest
}: NoticeProps) {
  const titleId = useId()
  const strip = shape === "strip"
  const stacked = !strip || title != null
  const look = TONE[tone]
  const buttons = actions?.map((a) => (
    <Button
      key={a.label}
      ref={a.buttonRef}
      size={strip ? "sm" : "default"}
      variant={LOOK[a.look ?? "default"]}
      disabled={a.disabled}
      autoFocus={a.autoFocus}
      data-testid={a.testId}
      onClick={a.onSelect}
      className={TOUCH_FLOOR}
    >
      {a.icon ? <a.icon /> : null}
      {a.label}
    </Button>
  ))
  const body = (
    // basis-40 lets a strip's buttons drop under the words on a narrow screen
    // instead of squeezing them to a sliver.
    <div className={cn("min-w-0 flex-1", strip && "basis-40")}>
      {title != null ? (
        <>
          <div id={titleId} className="font-semibold">
            {title}
          </div>
          <div className="mt-0.5">{children}</div>
        </>
      ) : (
        children
      )}
    </div>
  )
  const icon = Icon ? (
    <Icon aria-hidden className={cn("size-4 shrink-0", stacked && "mt-0.5", look.icon)} />
  ) : null
  return (
    <div
      {...rest}
      role={ROLE[tone]}
      aria-labelledby={title != null ? titleId : undefined}
      data-toast-clear={strip ? "" : undefined}
      className={cn(
        "shrink-0 text-sm",
        look.frame,
        strip
          ? cn(
              "flex flex-wrap gap-x-2 gap-y-2 border-b px-3 py-2",
              stacked ? "items-start" : "items-center",
            )
          : "flex flex-col gap-3 rounded-md border p-3",
        placement,
      )}
    >
      {strip ? (
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
          {buttons ? <div className="flex justify-end gap-3">{buttons}</div> : null}
        </>
      )}
    </div>
  )
}
