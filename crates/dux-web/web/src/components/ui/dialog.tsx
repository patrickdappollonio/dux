import * as React from "react"
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { useBackDismiss } from "@/hooks/use-back-dismiss"
import { useFocusFallback } from "@/lib/pageControl"
import { XIcon } from "lucide-react"

// Every dialog closes on the browser's Back, the way its Cancel does
// (`useBackDismiss`). `historyEntry={false}` takes a dialog out of that: one
// whose being open is already a position in the URL (the editor overlay), which
// Back leaves through the router instead, and one that opened by itself rather
// than from the user's own click or tap (a screen raised at boot, a server's
// answer, a target that vanished). Back must never be what dismisses those,
// and their entry would land in history with no gesture behind it.
//
// A dialog whose opener has gone when it closes hands focus to the page
// control that opened the menu it came from (`useFocusFallback`).
//
// `onBack` answers Back itself, for a dialog that may stay open and keep its
// entry, and `onBackStep` a Back that arrives while it has none
// (`useBackDismiss`).
function Dialog({
  historyEntry = true,
  onBack,
  onBackStep,
  ...props
}: DialogPrimitive.Root.Props & {
  historyEntry?: boolean
  onBack?: () => boolean
  onBackStep?: () => boolean
}) {
  const back = useBackDismiss(props, historyEntry, onBack, onBackStep)
  const onOpenChangeComplete = useFocusFallback(props.open, props.onOpenChangeComplete)
  return (
    <DialogPrimitive.Root
      data-slot="dialog"
      {...props}
      {...back}
      onOpenChangeComplete={onOpenChangeComplete}
    />
  )
}

function DialogTrigger({ ...props }: DialogPrimitive.Trigger.Props) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: DialogPrimitive.Portal.Props) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({ ...props }: DialogPrimitive.Close.Props) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogOverlay({
  className,
  ...props
}: DialogPrimitive.Backdrop.Props) {
  return (
    <DialogPrimitive.Backdrop
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 isolate z-50 bg-black/10 duration-100 supports-backdrop-filter:backdrop-blur-sm data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className
      )}
      {...props}
    />
  )
}

// max-h caps the card to the dynamic viewport, which the soft keyboard shrinks,
// and the card scrolls inside it. grid-cols-[minmax(0,1fr)] caps the column at
// the card's width, so a long unbreakable name cannot widen it and truncation
// inside engages.
const CARD_POPUP =
  "fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100dvh-2rem)] w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 grid-cols-[minmax(0,1fr)] gap-4 overflow-y-auto rounded-xl bg-popover p-4 text-sm text-popover-foreground ring-1 ring-foreground/10 duration-100 outline-none sm:max-w-sm data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95"

const COVER_POPUP =
  "fixed inset-0 z-50 flex flex-col bg-popover pt-[env(safe-area-inset-top)] text-sm text-popover-foreground duration-100 outline-none data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0"

// base-ui's focus trap does not stop the page behind focusing itself (a
// terminal that mounts again across a layout switch focuses its input), and the
// next key would then go there. A focus that lands behind the open popup is
// sent back to where it was inside. Behind is the page, or a layer this popup
// is nested in (the editor overlay under its own dialogs); a layer of its own
// (its focus guards, a menu it opened) or one opened over it is not. While
// another popup is open over this one, base-ui marks this one inert, and
// holding focus is that one's job.
function holdFocusInside(popup: HTMLElement): () => void {
  let last: HTMLElement | null = null
  const onFocusIn = (event: FocusEvent) => {
    const target = event.target
    if (!(target instanceof HTMLElement) || !popup.hasAttribute("data-open")) return
    if (popup.contains(target)) {
      last = target
      return
    }
    if (popup.closest("[data-base-ui-inert]") !== null) return
    const own = popup.closest("[data-base-ui-portal]")
    if (own?.contains(target)) return
    const layer = target.closest("[data-base-ui-portal]")
    if (layer !== null && !layer.contains(popup)) return
    const back = last !== null && last.isConnected && popup.contains(last) ? last : popup
    back.focus({ preventScroll: true })
  }
  document.addEventListener("focusin", onFocusIn, true)
  return () => document.removeEventListener("focusin", onFocusIn, true)
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  destructive = false,
  cover = false,
  ref,
  ...props
}: DialogPrimitive.Popup.Props & {
  showCloseButton?: boolean
  // The popup fills the screen edge to edge, as a column, rather than floating
  // as a card: nothing of the page shows around it.
  cover?: boolean
  // When true, the shared backdrop also desaturates the app behind it
  // (black-and-white) as an extra danger cue for consequential confirm dialogs.
  // Blur stays on the base overlay for every dialog regardless.
  destructive?: boolean
  ref?: React.Ref<HTMLDivElement>
}) {
  const release = React.useRef<(() => void) | null>(null)
  const popupRef = React.useCallback(
    (popup: HTMLDivElement | null) => {
      release.current?.()
      release.current = popup === null ? null : holdFocusInside(popup)
      if (typeof ref === "function") ref(popup)
      else if (ref) ref.current = popup
    },
    [ref]
  )
  return (
    <DialogPortal>
      {/* Forced, because base-ui draws no backdrop for a dialog nested inside
          another (the editor overlay's own dialogs), and without one a press
          outside lands on the dialog behind and closes nothing. */}
      <DialogOverlay
        forceRender
        className={destructive ? "backdrop-grayscale" : undefined}
      />
      <DialogPrimitive.Popup
        data-slot="dialog-content"
        className={cn(
          cover ? COVER_POPUP : CARD_POPUP,
          className
        )}
        ref={popupRef}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            render={
              // The 28px box and its focus ring stay put; at phone width and
              // under a coarse pointer an invisible margin widens the tap area
              // to 40px. It is 7px, not 6, because it is measured from inside
              // the button's 1px border.
              <Button
                variant="ghost"
                className="absolute top-2 right-2 after:absolute after:-inset-[7px] after:hidden max-md:after:block pointer-coarse:after:block"
                size="icon-sm"
              />
            }
          >
            <XIcon
            />
            <span className="sr-only">Close</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Popup>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      // Every footer action is at least 44px tall at phone width, and at
      // desktop width under a coarse pointer (a touch tablet, which is wide
      // enough for the desktop layout but still tapped with a finger). A fine
      // pointer at desktop width keeps the compact h-8 default. The selector
      // also takes a `Button` rendered as a link, which is no <button>.
      className={cn(
        "-mx-4 -mb-4 flex flex-col-reverse gap-2 rounded-b-xl border-t bg-muted/50 p-4 max-md:[&_:is(button,[data-slot=button])]:min-h-11 pointer-coarse:[&_:is(button,[data-slot=button])]:min-h-11 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close render={<Button variant="outline" />}>
          Close
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({ className, ...props }: DialogPrimitive.Title.Props) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "font-heading text-base leading-none font-medium",
        className
      )}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: DialogPrimitive.Description.Props) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-sm text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
