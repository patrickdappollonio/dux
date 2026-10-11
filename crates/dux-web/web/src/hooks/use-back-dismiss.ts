import { useEffect, useRef, useState, type RefObject } from "react"

import { registerModal } from "@/lib/modalHistory"

// The imperative handle a base-ui Root fills in: its `close()` runs the Root's
// own close.
interface Closable {
  close: () => void
}

// What a base-ui Root (Dialog, Menu, Popover, ContextMenu) takes that this hook
// needs: its open state, controlled or not, and its imperative handle.
interface RootOpenProps<Details extends { isCanceled: boolean }, Actions extends Closable> {
  open?: boolean
  defaultOpen?: boolean
  onOpenChange?: (open: boolean, details: Details) => void
  actionsRef?: RefObject<Actions | null>
}

// Lets the browser's Back (and a phone's back gesture) close a shared dialog,
// sheet or menu primitive. While the Root is open it is registered in
// `modalHistory`, which the store gives a history entry of its own; Back closes
// it through the Root's own imperative close, so the caller's `onOpenChange`
// hears `false` exactly as it does for Cancel or Escape and abandons whatever
// the modal was offering. Returns the two props to pass on to the Root in place
// of the caller's. With `enabled` false the Root is never registered.
export function useBackDismiss<
  Details extends { isCanceled: boolean },
  Actions extends Closable,
>(
  props: RootOpenProps<Details, Actions>,
  enabled = true,
): {
  actionsRef: RefObject<Actions | null>
  onOpenChange: (open: boolean, details: Details) => void
} {
  const { open, defaultOpen, onOpenChange, actionsRef: callerRef } = props
  const localRef = useRef<Actions | null>(null)
  // A caller-supplied handle wins, so both of them reach the same Root.
  const actionsRef = callerRef ?? localRef
  // An uncontrolled Root keeps its open state inside base-ui, so it is
  // mirrored here from the changes base-ui reports and does not cancel.
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen ?? false)
  const isOpen = open ?? uncontrolledOpen

  useEffect(() => {
    if (!isOpen || !enabled) return
    return registerModal(() => actionsRef.current?.close())
  }, [isOpen, enabled, actionsRef])

  const handleOpenChange = (next: boolean, details: Details) => {
    onOpenChange?.(next, details)
    if (!details.isCanceled) setUncontrolledOpen(next)
  }
  return { actionsRef, onOpenChange: handleOpenChange }
}
