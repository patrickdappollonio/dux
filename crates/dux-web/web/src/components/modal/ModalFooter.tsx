import { Fragment, type MouseEvent, type ReactNode, type Ref } from "react"

import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { GlyphSpinner } from "@/components/GlyphSpinner"
import { Button } from "@/components/ui/button"
import { FOCUS_RING } from "@/lib/focusRing"
import { cn } from "@/lib/utils"

import { COVER_BOTTOM_INSET } from "./layout"
import type { ModalAction, ModalDoor, ModalTone } from "./types"

const VARIANT = {
  primary: "default",
  destructive: "destructive",
  plain: "outline",
} as const satisfies Record<ModalTone, string>

/** The button that only leaves: a confirm's or a form's Cancel, a report's Close. */
export interface SafeButton {
  label: string
  onSelect: () => void
  disabled: boolean
}

// 44px tall where a finger presses it. In a stacked footer the label may wrap
// instead of truncating, since each button has half the row.
const ROW_BUTTON = "pointer-coarse:min-h-11"
const STACKED_BUTTON = "h-auto min-h-11 w-full py-2 text-center whitespace-normal"

function ActionButton({
  action,
  stacked,
  wide,
}: {
  action: ModalAction
  stacked: boolean
  wide: boolean
}) {
  const reason = action.disabledReason ?? null
  const Icon = action.icon
  return (
    <ExplainedWhenDisabled
      disabled={reason !== null}
      hint={reason}
      className={cn("rounded-lg", stacked && wide && "col-span-2")}
    >
      <Button
        variant={VARIANT[action.tone]}
        disabled={reason !== null || action.busy === true}
        aria-busy={action.busy === true ? true : undefined}
        className={stacked ? STACKED_BUTTON : ROW_BUTTON}
        onClick={(event: MouseEvent) => {
          // The second click of a double click lands on whatever replaced the
          // button under it, and must not confirm that too.
          if (action.tone === "destructive" && event.detail > 1) return
          action.onSelect()
        }}
      >
        {action.busy === true ? <GlyphSpinner /> : Icon ? <Icon /> : null}
        {action.label}
      </Button>
    </ExplainedWhenDisabled>
  )
}

function SafeFooterButton({
  safe,
  buttonRef,
  stacked,
  wide,
}: {
  safe: SafeButton
  buttonRef: Ref<HTMLButtonElement>
  stacked: boolean
  wide: boolean
}) {
  // Wrapped like an action so that every footer button is the same kind of
  // grid cell, whichever it is.
  return (
    <span className={cn("inline-flex rounded-lg", stacked && wide && "col-span-2")}>
      <Button
        ref={buttonRef}
        variant="outline"
        disabled={safe.disabled}
        className={stacked ? STACKED_BUTTON : ROW_BUTTON}
        onClick={safe.onSelect}
      >
        {safe.label}
      </Button>
    </span>
  )
}

/** The footer of a confirm, a form or a report. Stacked, two buttons share one
 * row as halves, and with three the safe one has the row below to itself. */
export function ModalFooter({
  stacked,
  cover,
  safe,
  safeRef,
  actions,
  note,
}: {
  stacked: boolean
  cover: boolean
  safe: SafeButton | null
  safeRef: Ref<HTMLButtonElement>
  /** In reading order; the last is the main one. */
  actions: readonly ModalAction[]
  note?: ReactNode
}) {
  const count = actions.length + (safe === null ? 0 : 1)
  if (count === 0) return null
  const buttons = actions.map((action, index) => (
    <ActionButton
      key={index}
      action={action}
      stacked={stacked}
      wide={count === 1}
    />
  ))
  const safeButton =
    safe === null ? null : (
      <SafeFooterButton safe={safe} buttonRef={safeRef} stacked={stacked} wide={count !== 2} />
    )
  if (!stacked) {
    return (
      <div
        data-slot="modal-footer"
        className="flex shrink-0 items-center justify-end gap-2 rounded-b-xl border-t bg-muted/50 p-4"
      >
        {note ? (
          <div className="mr-auto min-w-0 text-xs text-muted-foreground">{note}</div>
        ) : null}
        {safeButton}
        {buttons}
      </div>
    )
  }
  return (
    <div
      data-slot="modal-footer"
      className={cn(
        "flex shrink-0 flex-col gap-2 border-t bg-muted/50 px-4 pt-3",
        cover ? COVER_BOTTOM_INSET : "rounded-b-xl pb-4",
      )}
    >
      {note ? <div className="text-xs text-muted-foreground">{note}</div> : null}
      <div className="grid grid-cols-2 gap-3">
        {count === 2 ? safeButton : null}
        {buttons}
        {count === 2 ? null : safeButton}
      </div>
    </div>
  )
}

/** A picker's footer. Its doors are quieter than the rows above: they leave
 * the list rather than pick from it. */
export function ModalDoors({
  doors,
  phone,
  cover,
}: {
  doors: readonly ModalDoor[]
  phone: boolean
  cover: boolean
}) {
  return (
    <div
      data-slot="modal-footer"
      className={cn(
        "grid shrink-0 gap-3 border-t px-2 pt-2",
        doors.length > 1 ? "grid-cols-[1fr_auto_1fr]" : "grid-cols-1",
        cover ? COVER_BOTTOM_INSET : "pb-2",
      )}
    >
      {doors.map((door, index) => (
        <Fragment key={index}>
          {index > 0 ? (
            <div
              data-slot="footer-divider"
              aria-hidden="true"
              className="w-px self-stretch bg-border"
            />
          ) : null}
          <DoorButton door={door} phone={phone} />
        </Fragment>
      ))}
    </div>
  )
}

// Not a `Button`: its label is left-aligned, muted and regular weight. The
// transparent border is what the focus ring's border half colours.
function DoorButton({ door, phone }: { door: ModalDoor; phone: boolean }) {
  const reason = door.disabledReason ?? null
  const Icon = door.icon
  return (
    <ExplainedWhenDisabled disabled={reason !== null} hint={reason} className="min-w-0">
      <button
        type="button"
        disabled={reason !== null}
        onClick={door.onSelect}
        className={cn(
          "flex min-w-0 flex-1 items-center gap-2.5 rounded-md border border-transparent px-[7px] text-left text-sm text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground disabled:pointer-events-none disabled:opacity-50",
          // On a phone the label wraps rather than truncating in half a row.
          phone ? "min-h-11 py-1 leading-tight" : "h-9 pointer-coarse:h-10",
          FOCUS_RING,
        )}
      >
        <Icon className="size-4 shrink-0" />
        <span className={cn("min-w-0", !phone && "truncate")}>{door.label}</span>
      </button>
    </ExplainedWhenDisabled>
  )
}
