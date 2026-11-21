import type { ReactNode } from "react"
import { X } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet"

// A form's popup under a `Dialog` root: a dialog on a computer, and on a phone
// a full-height bottom sheet whose title row carries a close button, whose body
// scrolls between it and a footer pinned to the bottom, so the footer's
// buttons are on screen whatever the body holds and however short the screen.
// `footer` lists its buttons Cancel first; Save then sits on top on a phone.
export function FormDialogContent({
  phone,
  title,
  description,
  footer,
  onClose,
  className,
  children,
}: {
  phone: boolean
  title: ReactNode
  description: ReactNode
  /** Absent while there is nothing to act on yet. */
  footer?: ReactNode
  onClose: () => void
  /** The computer dialog's classes. */
  className?: string
  children: ReactNode
}) {
  if (phone) {
    return (
      <SheetContent
        side="bottom"
        showCloseButton={false}
        fullHeight
        className="gap-0 rounded-t-2xl pb-[env(safe-area-inset-bottom)]"
      >
        <div className="flex min-h-16 shrink-0 items-center gap-2 border-b py-3 pr-3 pl-4">
          <SheetTitle className="min-w-0 flex-1">{title}</SheetTitle>
          <Button
            variant="ghost"
            size="icon"
            className="size-10 shrink-0"
            aria-label="Close"
            onClick={onClose}
          >
            <X />
          </Button>
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
          <SheetDescription>{description}</SheetDescription>
          {children}
        </div>
        {footer ? (
          <div className="flex shrink-0 flex-col-reverse gap-2 border-t bg-muted/50 px-4 pt-3 pb-4 [&_:is(button,[data-slot=button])]:min-h-11">
            {footer}
          </div>
        ) : null}
      </SheetContent>
    )
  }
  return (
    <DialogContent showCloseButton={false} className={className}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {children}
      {footer ? <DialogFooter>{footer}</DialogFooter> : null}
    </DialogContent>
  )
}
