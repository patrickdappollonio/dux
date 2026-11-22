import type { LucideIcon } from "lucide-react"
import type { ReactNode, RefObject } from "react"

/** The width a modal's card may reach on a computer; a phone's cover ignores it. */
export type ModalSize = "sm" | "md" | "lg" | "xl" | "2xl" | "3xl" | "4xl"

export type ModalTone = "primary" | "destructive" | "plain"

/** One footer button, as data: the modal draws it, sizes it and places it. */
export interface ModalAction {
  label: string
  onSelect: () => void
  tone: ModalTone
  icon?: LucideIcon
  /** Shows a spinner and takes no press. */
  busy?: boolean
  /** Non-null disables the button and is what its hint says. */
  disabledReason?: string | null
}

/** A picker's way out to somewhere else, drawn quieter than a row. */
export interface ModalDoor {
  label: string
  icon: LucideIcon
  onSelect: () => void
  disabledReason?: string | null
}

/** A picker's search field: the modal draws it and, under a fine pointer, focuses it. */
export interface ModalSearch {
  value: string
  onChange: (value: string) => void
  /** The field's placeholder and its accessible name. */
  label: string
  /** Extra props for the input, such as `useSearchPick`'s. */
  fieldProps?: Omit<React.ComponentProps<"input">, "value" | "onChange" | "className">
}

/** What the shared discard confirm says: `holder` opens the sentence. */
export interface ModalUnsavedEdits {
  holder: string
  changes: number
}

interface ModalBase {
  open: boolean
  /** Every way out that is not an action: Cancel, Escape, the close button, a
   * press outside and the browser's Back. */
  onClose: () => void
  title: ReactNode
  description?: ReactNode
  children?: ReactNode
  size?: ModalSize
  /** False for a modal that opened by itself: Back must not be what answers it. */
  historyEntry?: boolean
  /** Refuses every way out while true, and Back closes once it is false. */
  busy?: boolean
  finalFocus?: RefObject<HTMLElement | null> | (() => HTMLElement | boolean)
}

interface ModalSteps {
  /** The way back to the step before this one. */
  back?: { label: string; onSelect: () => void }
  /** Names the step on screen; a change of step is focused as an open is. */
  step?: string
  /** What takes focus on open in place of the kind's own choice. Under a
   * coarse pointer a field text is typed into is still not focused. */
  initialFocus?: RefObject<HTMLElement | null>
}

export interface ConfirmModalProps extends ModalBase {
  kind: "confirm"
  /** The safe button: it has focus on open and does what `onClose` does. */
  cancelLabel: string
  confirm: ModalAction
  /** A second way forward, between the safe button and `confirm`. */
  alternative?: ModalAction
}

export interface FormModalProps extends ModalBase, ModalSteps {
  kind: "form"
  submit: ModalAction
  secondary?: ModalAction
  cancelLabel?: string
  footerNote?: ReactNode
  /** With changes above zero, every way out asks before the edits are lost. */
  unsavedEdits?: ModalUnsavedEdits | null
}

export interface PickerModalProps extends ModalBase, ModalSteps {
  kind: "picker"
  search?: ModalSearch
  /** The list keeps one height on a computer whatever a search leaves in it. */
  steadyHeight?: boolean
  doors?: readonly [ModalDoor] | readonly [ModalDoor, ModalDoor]
}

export interface ReportModalProps extends ModalBase, ModalSteps {
  kind: "report"
  actions?: readonly [ModalAction] | readonly [ModalAction, ModalAction]
  footerNote?: ReactNode
}

export type ModalProps =
  | ConfirmModalProps
  | FormModalProps
  | PickerModalProps
  | ReportModalProps

export type ModalKind = ModalProps["kind"]
