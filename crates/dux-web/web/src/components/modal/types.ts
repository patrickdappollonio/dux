import type { LucideIcon } from "lucide-react"
import type { ReactNode, Ref, RefObject } from "react"

import type { Prose } from "@/lib/prose"

/** The width a card may reach; a cover ignores it. `sm` to `4xl` are
 * Tailwind's `max-w-*` widths, and a `w` token is that many pixels. */
export type ModalSize =
  | "sm"
  | "md"
  | "lg"
  | "xl"
  | "w600"
  | "w640"
  | "w700"
  | "w704"
  | "3xl"
  | "w880"
  | "4xl"
  | "w940"

/** A card's height: its content's, or fixed at that many pixels (less on a
 * short screen) so a body with panes of its own fills it. A cover ignores it. */
export type ModalHeight = "content" | "h640" | "h680"

/** Where a card sits: centred, or hung from near the top, so a list that
 * grows and shrinks leaves the header where it was. A cover ignores it. */
export type ModalAnchor = "center" | "top"

export type ModalTone = "primary" | "destructive" | "plain"

interface ActionBase {
  label: string
  tone: ModalTone
  icon?: LucideIcon
  /** Non-null disables the action and is what its hint says. */
  disabledReason?: string | null
}

/** One footer button, as data: the modal draws it, sizes it and places it. */
export interface ModalButtonAction extends ActionBase {
  onSelect: () => void
  /** Shows a spinner and takes no press; the button keeps keyboard focus. */
  busy?: boolean
  href?: undefined
}

/** A footer action that is a link: it opens `href` in a new tab, and a middle
 * click or the browser's own link menu works on it. */
export interface ModalLinkAction extends ActionBase {
  href: string
  /** Runs on a press, before the browser follows the link. */
  onSelect?: () => void
  busy?: undefined
}

export type ModalAction = ModalButtonAction | ModalLinkAction

/** A picker's way out to somewhere else, drawn quieter than a row. */
export interface ModalDoor {
  label: string
  icon: LucideIcon
  onSelect: () => void
  disabledReason?: string | null
}

/** A picker's search field: the modal draws it and, under a fine pointer,
 * focuses it. */
export interface ModalSearch {
  value: string
  onChange: (value: string) => void
  /** The field's placeholder and its accessible name. */
  label: string
  /** `useSearchPick`'s keyboard and focus handlers and the row it marks.
   * The field's type, look and element are the modal's. */
  fieldProps?: Pick<
    React.ComponentProps<"input">,
    "onKeyDown" | "onFocus" | "onBlur" | "aria-activedescendant"
  >
}

/** A form's edits that closing would lose. The question reads "`holder` N
 * unsaved changes. Discarding them leaves `leaves` as it was." */
export interface ModalUnsavedEdits {
  /** Zero asks nothing. */
  changes: number
  /** Opens the sentence, with its verb: "The macros have". */
  holder: string
  /** What stays untouched, a name in it as a chip. */
  leaves: Prose
  /** Keeps the edits somewhere the user can get them back from. With it, Back
   * closes at once and calls this rather than asking. */
  setAside?: () => void
}

/** The way out that was taken. */
export type ModalCloseReason =
  /** The safe button: Cancel, or a report's Close. */
  | "cancel"
  /** The corner close or a cover's title-row close. */
  | "close-control"
  | "escape"
  /** A press outside the card. */
  | "outside"
  /** The browser's Back, or a phone's back gesture. */
  | "back"
  /** `vanished` turned true. */
  | "vanished"

interface ModalBase {
  open: boolean
  /** The modal is closing by a way out that is not an action; the caller
   * turns `open` false. */
  onClose: (reason: ModalCloseReason) => void
  /** Asked before every way out but a vanished target, and before the
   * unsaved-edits question. "stay" keeps the modal open: the caller answered
   * the press some other way. */
  beforeClose?: (reason: ModalCloseReason) => "close" | "stay"
  /** True once what the modal is about is gone: it closes at once, busy or
   * not, and asks nothing. */
  vanished?: boolean
  /** The close has finished animating: what holds the modal's state may go. */
  onClosed?: () => void
  title: ReactNode
  description?: ReactNode
  children?: ReactNode
  size?: ModalSize
  /** False for a modal that opened by itself: Back must not be what answers it. */
  historyEntry?: boolean
  /** Refuses every way out while true; a Back that arrived meanwhile closes
   * the modal once it is false. */
  busy?: boolean
  finalFocus?: RefObject<HTMLElement | null> | (() => HTMLElement | boolean)
}

interface ModalFrame {
  /** The way back to the step before this one. */
  back?: { label: string; onSelect: () => void }
  /** Names the step on screen; a change of step is focused as an open is. */
  step?: string
  /** What takes focus on open in place of the kind's own choice. Under a
   * coarse pointer a field text is typed into is still not focused. */
  initialFocus?: RefObject<HTMLElement | null> | (() => HTMLElement | null)
  /** A row between the header and the body that does not scroll with the
   * body: tabs, a sort control, a filter. */
  toolbar?: ReactNode
  height?: ModalHeight
  anchor?: ModalAnchor
}

export interface ConfirmModalProps extends ModalBase {
  kind: "confirm"
  /** The safe button: it has focus on open and does what `onClose` does. */
  cancelLabel: string
  confirm: ModalAction
  /** A second way forward, between the safe button and `confirm`. */
  alternative?: ModalAction
  /** The safe button's element, for a caller that puts focus back on it. */
  safeRef?: Ref<HTMLButtonElement>
}

export interface FormModalProps extends ModalBase, ModalFrame {
  kind: "form"
  submit: ModalAction
  secondary?: ModalAction
  cancelLabel?: string
  footerNote?: ReactNode
  /** With changes above zero, every way out asks before the edits are lost. */
  unsavedEdits?: ModalUnsavedEdits | null
  safeRef?: Ref<HTMLButtonElement>
}

export interface PickerModalProps extends ModalBase, ModalFrame {
  kind: "picker"
  search?: ModalSearch
  /** The list keeps one height on a card whatever a search leaves in it. */
  steadyHeight?: boolean
  doors?: readonly [ModalDoor, ...ModalDoor[]]
}

export interface ReportModalProps extends ModalBase, ModalFrame {
  kind: "report"
  /** In reading order. A destructive one starts the footer's row, apart from
   * the Close that ends it. */
  actions?: readonly ModalAction[]
  footerNote?: ReactNode
  /** What a card's safe button says; "Close" unless given. */
  closeLabel?: string
  safeRef?: Ref<HTMLButtonElement>
}

export type ModalProps =
  | ConfirmModalProps
  | FormModalProps
  | PickerModalProps
  | ReportModalProps

export type ModalKind = ModalProps["kind"]
