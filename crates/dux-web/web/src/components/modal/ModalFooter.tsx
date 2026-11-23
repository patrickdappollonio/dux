import type { CSSProperties, MouseEvent, ReactNode, Ref } from "react"

import { ExplainedWhenDisabled } from "@/components/ExplainedWhenDisabled"
import { Spinner } from "@/components/Spinner"
import { Button, buttonVariants } from "@/components/ui/button"
import { FOCUS_RING } from "@/lib/focusRing"
import { cn } from "@/lib/utils"

import { COVER_BOTTOM_INSET, INERT_BUTTON } from "./layout"
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
  /** The modal is busy: the button takes no press and keeps its focus. */
  inert: boolean
}

// 44px tall where a finger presses it. In a stacked footer the label may wrap
// instead of truncating, since each button has half the row.
const ROW_BUTTON = "pointer-coarse:min-h-11"
const STACKED_BUTTON = "h-auto min-h-11 w-full py-2 text-center whitespace-normal"

function ActionButton({
  action,
  stacked,
  modalBusy,
  cellClassName,
}: {
  action: ModalAction
  stacked: boolean
  /** The whole modal is busy: no action takes a press, and each keeps its focus. */
  modalBusy: boolean
  cellClassName?: string
}) {
  const reason = action.disabledReason ?? null
  const busy = action.busy === true
  const Icon = action.icon
  const content = (
    <>
      {busy ? <Spinner size="fit" tone="current" /> : Icon ? <Icon /> : null}
      {action.label}
    </>
  )
  const className = cn(
    stacked ? STACKED_BUTTON : ROW_BUTTON,
    INERT_BUTTON,
    // A filled button's fill stops inside its transparent border, so it paints
    // 2px shorter than the outlined Cancel beside it unless it fills the border.
    action.tone !== "plain" && "bg-clip-border",
  )
  return (
    <ExplainedWhenDisabled
      disabled={reason !== null}
      hint={reason}
      className={cn("rounded-lg", cellClassName)}
    >
      {action.href !== undefined && reason === null && !modalBusy ? (
        <a
          data-slot="button"
          href={action.href}
          target="_blank"
          rel="noopener noreferrer"
          onClick={action.onSelect}
          className={cn(buttonVariants({ variant: VARIANT[action.tone] }), className)}
        >
          {content}
        </a>
      ) : (
        <Button
          variant={VARIANT[action.tone]}
          // Only a reason takes the button out of the tab order: its wrapper
          // takes the focus then. A busy one keeps the focus it has.
          disabled={reason !== null || busy || modalBusy}
          focusableWhenDisabled={reason === null}
          aria-busy={busy ? true : undefined}
          className={className}
          onClick={(event: MouseEvent) => {
            // The second click of a double click lands on whatever replaced the
            // button under it, and must not confirm that too.
            if (action.tone === "destructive" && event.detail > 1) return
            action.onSelect?.()
          }}
        >
          {content}
        </Button>
      )}
    </ExplainedWhenDisabled>
  )
}

function SafeFooterButton({
  safe,
  buttonRef,
  stacked,
  cellClassName,
}: {
  safe: SafeButton
  buttonRef?: Ref<HTMLButtonElement>
  stacked: boolean
  cellClassName?: string
}) {
  // Wrapped like an action so that every footer button is the same kind of
  // grid cell, whichever it is.
  return (
    <span className={cn("inline-flex rounded-lg", cellClassName)}>
      <Button
        ref={buttonRef}
        data-modal-safe=""
        variant="outline"
        disabled={safe.inert}
        focusableWhenDisabled
        className={cn(stacked ? STACKED_BUTTON : ROW_BUTTON, INERT_BUTTON)}
        onClick={safe.onSelect}
      >
        {safe.label}
      </Button>
    </span>
  )
}

/** The footer of a confirm, a form or a report. In a row the main button ends
 * it. Stacked, two buttons share one row as halves, and with three the safe
 * one has the row below to itself. */
export function ModalFooter({
  stacked,
  cover,
  safe,
  safeRef,
  safeEnds,
  leading,
  actions,
  note,
  busy,
}: {
  stacked: boolean
  cover: boolean
  safe: SafeButton | null
  safeRef?: Ref<HTMLButtonElement>
  /** The safe button is the main one and ends the row: a report's Close. */
  safeEnds: boolean
  /** Actions that start the row, apart from the rest: a report's destructive ones. */
  leading: readonly ModalAction[]
  /** In reading order; the last is the main one unless the safe button is. */
  actions: readonly ModalAction[]
  note?: ReactNode
  /** The modal is busy. */
  busy: boolean
}) {
  const count = leading.length + actions.length + (safe === null ? 0 : 1)
  if (count === 0) return null
  const hasNote = note !== undefined && note !== null && note !== false
  const cell = (wide: boolean, first: boolean) =>
    stacked
      ? wide
        ? "col-span-2"
        : undefined
      : first
        ? cn("order-first", !hasNote && "mr-auto")
        : undefined
  const safeFirst = stacked ? count === 2 : !safeEnds
  const safeButton =
    safe === null ? null : (
      <SafeFooterButton
        key="safe"
        safe={safe}
        buttonRef={safeRef}
        stacked={stacked}
        cellClassName={cell(count !== 2, false)}
      />
    )
  return (
    <div
      data-slot="modal-footer"
      className={cn(
        "flex shrink-0 border-t bg-muted/50",
        stacked
          ? cn("flex-col gap-2 px-4 pt-3", cover ? COVER_BOTTOM_INSET : "rounded-b-xl pb-4")
          : "items-center justify-end gap-2 rounded-b-xl p-4",
      )}
    >
      {hasNote ? (
        <div className={cn("min-w-0 text-xs text-muted-foreground", !stacked && "mr-auto")}>
          {note}
        </div>
      ) : null}
      {/* In a row the buttons are the footer's own flex items. */}
      <div className={stacked ? "grid grid-cols-2 gap-3" : "contents"}>
        {leading.map((action, index) => (
          <ActionButton
            key={`leading-${index}`}
            action={action}
            stacked={stacked}
            modalBusy={busy}
            cellClassName={cell(count === 1, index === 0)}
          />
        ))}
        {safeFirst ? safeButton : null}
        {actions.map((action, index) => (
          <ActionButton
            key={index}
            action={action}
            stacked={stacked}
            modalBusy={busy}
            cellClassName={cell(count === 1, false)}
          />
        ))}
        {safeFirst ? null : safeButton}
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
  // A phone stacks them one to a row, where half its width cannot hold a
  // door's icon and label on one line; a computer gives every door an equal
  // column, with a rule between neighbours. The stacked doors touch, as a
  // menu's items do (the user's choice of 2026-11-22): each is a full-width row
  // at least 44px tall, over the 40px touch floor, so a tap on one cannot land
  // on the other, and a gap would add nothing to telling them apart.
  const ruled = !phone && doors.length > 1
  const columns: CSSProperties | undefined =
    ruled && doors.length > 2
      ? { gridTemplateColumns: doors.map(() => "minmax(0,1fr)").join(" auto ") }
      : undefined
  return (
    <div
      data-slot="modal-footer"
      style={columns}
      className={cn(
        "grid shrink-0 border-t px-2 pt-2",
        phone ? "gap-0" : "gap-3",
        doors.length === 1 || phone
          ? "grid-cols-1"
          : doors.length === 2 && "grid-cols-[1fr_auto_1fr]",
        cover ? COVER_BOTTOM_INSET : "pb-2",
      )}
    >
      {doors.flatMap((door, index) => [
        index > 0 && ruled ? (
          <div
            key={`rule-${index}`}
            data-slot="footer-divider"
            aria-hidden="true"
            className="w-px self-stretch bg-border"
          />
        ) : null,
        <DoorButton key={index} door={door} phone={phone} />,
      ])}
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
          // On a phone a label too long for its row wraps rather than truncating.
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
