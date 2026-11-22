import { ChevronLeft, Search, X } from "lucide-react"
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from "react"

import { ConfirmDiscardEditsDialog } from "@/components/ConfirmDiscardEditsDialog"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useIsMobile } from "@/hooks/use-mobile"
import { cn } from "@/lib/utils"

import { focusTarget } from "./focus"
import {
  CARD_ANCHOR,
  CARD_HEIGHT,
  CARD_WIDTH,
  COVER_BOTTOM_INSET,
  INERT_BUTTON,
  modalPresentation,
} from "./layout"
import { ModalDoors, ModalFooter, type SafeButton } from "./ModalFooter"
import type { ModalAction, ModalCloseReason, ModalProps, ModalSearch } from "./types"

/** Every modal in the web UI. The caller hands over its kind and its parts;
 * the kind decides how it is presented, focused, laid out and closed. */
export function Modal(props: ModalProps) {
  const phone = useIsMobile()
  const { onClose, kind } = props
  const vanished = props.vanished === true
  const open = props.open && !vanished
  const busy = props.busy === true
  const unsaved = kind === "form" ? (props.unsavedEdits ?? null) : null
  // The way out that raised the discard question, while it is up.
  const [asked, setAsked] = useState<ModalCloseReason | null>(null)
  const [wasOpen, setWasOpen] = useState(open)
  if (wasOpen !== open) {
    setWasOpen(open)
    if (!open) setAsked(null)
  }
  const placeFocus = useRef<(() => HTMLElement | null) | null>(null)
  // A Back that arrived while busy, answered once the modal is free.
  const backWaits = useRef(false)

  /** Takes a way out, and says whether the modal is closing. */
  const leave = (reason: ModalCloseReason): boolean => {
    if (busy) {
      if (reason === "back") backWaits.current = true
      return false
    }
    if (asked !== null) {
      // Back abandons the question on top, as its Keep editing does.
      if (reason === "back") setAsked(null)
      return false
    }
    if (props.beforeClose?.(reason) === "stay") return false
    if (unsaved !== null && unsaved.changes > 0) {
      if (reason !== "back" || unsaved.setAside === undefined) {
        setAsked(reason)
        return false
      }
      unsaved.setAside()
    }
    onClose(reason)
    return true
  }

  useEffect(() => {
    if (props.open && vanished) onClose("vanished")
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.open, vanished])
  useEffect(() => {
    if (!open) backWaits.current = false
    if (!open || busy || !backWaits.current) return
    backWaits.current = false
    leave("back")
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, busy])

  const presentation = modalPresentation(kind, phone)
  const frame = kind === "confirm" ? null : props
  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(next, details) => {
          if (next) return
          if (!leave(details.reason === "escape-key" ? "escape" : "outside")) details.cancel()
        }}
        onBack={() => !leave("back")}
        onOpenChangeComplete={(now) => {
          if (!now) props.onClosed?.()
        }}
        historyEntry={props.historyEntry !== false}
      >
        <DialogContent
          cover={presentation === "cover"}
          data-kind={kind}
          data-presentation={presentation}
          showCloseButton={false}
          destructive={kind === "confirm" && props.confirm.tone === "destructive"}
          finalFocus={props.finalFocus}
          // False leaves focus where it is; true would move it to the popup's
          // first control. Only a popup whose parts have not mounted gets that.
          initialFocus={() => (placeFocus.current === null ? true : (placeFocus.current() ?? false))}
          className={cn(
            presentation === "card" &&
              cn(
                "flex flex-col gap-0 p-0",
                CARD_WIDTH[props.size ?? "sm"],
                CARD_HEIGHT[frame?.height ?? "content"],
                CARD_ANCHOR[frame?.anchor ?? "center"],
              ),
          )}
        >
          <ModalParts
            {...props}
            phone={phone}
            leave={leave}
            onFocusTarget={(target) => {
              placeFocus.current = target
            }}
          />
        </DialogContent>
      </Dialog>
      {unsaved !== null ? (
        <ConfirmDiscardEditsDialog
          open={open && asked !== null}
          holder={unsaved.holder}
          changes={unsaved.changes}
          leaves={unsaved.leaves}
          onKeep={() => setAsked(null)}
          onDiscard={() => {
            const reason = asked ?? "cancel"
            setAsked(null)
            onClose(reason)
          }}
        />
      ) : null}
    </>
  )
}

function footerActions(props: ModalProps): readonly ModalAction[] {
  switch (props.kind) {
    case "confirm":
      return props.alternative ? [props.alternative, props.confirm] : [props.confirm]
    case "form":
      return props.secondary ? [props.secondary, props.submit] : [props.submit]
    case "report":
      return props.actions ?? []
    case "picker":
      return []
  }
}

// The popup's contents. A component of its own so that it mounts with the
// popup, and its layout effect runs after everything in the body has mounted.
//
// A cover and a card are the same elements in the same places with other
// classes, so a switch of layout under an open modal remounts nothing.
function ModalParts(
  props: ModalProps & {
    phone: boolean
    leave: (reason: ModalCloseReason) => boolean
    /** Handed the function that says what takes focus now, on every render. */
    onFocusTarget: (target: () => HTMLElement | null) => void
  },
) {
  const { kind, phone, leave, open, onFocusTarget } = props
  const cover = modalPresentation(kind, phone) === "cover"
  const coarse = useIsCoarsePointer()
  const busy = props.busy === true
  const header = useRef<HTMLDivElement | null>(null)
  const body = useRef<HTMLDivElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)

  const frame = kind === "confirm" ? null : props
  const back = frame?.back
  const step = frame?.step
  const callerSafeRef = kind === "picker" ? undefined : props.safeRef
  const target = () => {
    const popup = header.current?.closest<HTMLElement>('[data-slot="dialog-content"]')
    if (!popup) return null
    const named = frame?.initialFocus
    return focusTarget({
      kind,
      coarse,
      popup,
      body: body.current,
      named: (typeof named === "function" ? named() : named?.current) ?? null,
      safeButton: popup.querySelector<HTMLElement>("[data-modal-safe]"),
      search: searchRef.current,
    })
  }
  useLayoutEffect(() => {
    onFocusTarget(target)
  })
  const last = useRef<{ open: boolean; step: string | undefined } | null>(null)
  useLayoutEffect(() => {
    const was = last.current
    last.current = { open, step }
    if (!open) return
    const popup = header.current?.closest('[data-slot="dialog-content"]')
    const stepChanged = was !== null && was.open && was.step !== step
    // On open the popup places focus itself, unless a control inside already
    // took it; that case and a change of step are placed here.
    if (!stepChanged && !popup?.contains(document.activeElement)) return
    target()?.focus({ preventScroll: true })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, step])

  // A report's Close is drawn on a card only, since a cover's title row closes
  // it; a form keeps Cancel on both, because Cancel abandons what was typed.
  const safeLabel =
    kind === "confirm"
      ? props.cancelLabel
      : kind === "form"
        ? (props.cancelLabel ?? "Cancel")
        : kind === "report" && !cover
          ? (props.closeLabel ?? "Close")
          : null
  const safe: SafeButton | null =
    safeLabel === null ? null : { label: safeLabel, onSelect: () => leave("cancel"), inert: busy }

  const actions = footerActions(props)
  // A report's destructive action starts the row, apart from its Close.
  const leading = kind === "report" ? actions.filter((a) => a.tone === "destructive") : []
  const hasFooter = kind === "picker" ? props.doors !== undefined : safe !== null || actions.length > 0
  const hasBody = props.children !== undefined && props.children !== null && props.children !== false
  const hasSearch = kind === "picker" && props.search !== undefined
  const steady = kind === "picker" && props.steadyHeight === true
  const fixedHeight = !cover && (frame?.height ?? "content") !== "content"

  const description = props.description ? (
    <DialogDescription className="text-pretty">{props.description}</DialogDescription>
  ) : null
  // A cover's description scrolls with the body, under the title row.
  const bodyDescription = !cover ? null : kind === "picker" && description ? (
    <div className="px-2 pt-1 pb-2">{description}</div>
  ) : (
    description
  )
  const bodyClass = cn(
    kind === "picker" ? "p-2" : "flex flex-col gap-4",
    kind !== "picker" && (cover ? "p-4" : "px-4 pt-1 pb-6"),
    cover && !hasFooter && COVER_BOTTOM_INSET,
  )
  return (
    <>
      <div
        ref={header}
        data-slot="modal-header"
        className={
          cover
            ? "flex min-h-14 shrink-0 items-center gap-1 border-b py-2 pr-2 pl-4"
            : cn(
                "flex shrink-0 flex-col gap-2 p-4",
                hasSearch ? "pb-0" : kind === "picker" ? "pb-3" : !hasBody && "pb-6",
              )
        }
      >
        {back ? (
          // Quieter than the modal's actions: it navigates. On a card it is
          // 32px, as its only neighbour is the corner close across the header.
          <Button
            variant="ghost"
            size={cover ? "icon" : "sm"}
            aria-label={`Back to ${back.label}`}
            onClick={back.onSelect}
            className={
              cover
                ? "-ml-2 size-10 shrink-0"
                : "-mt-1 -ml-1.5 h-8 self-start px-1.5 text-muted-foreground pointer-coarse:min-h-10"
            }
          >
            <ChevronLeft />
            {cover ? null : back.label}
          </Button>
        ) : null}
        <DialogTitle
          className={cn("leading-snug", cover ? "min-w-0 flex-1" : kind === "picker" && "pr-8")}
        >
          {props.title}
        </DialogTitle>
        {cover || kind === "picker" ? (
          // The card's 28px box keeps its place; at phone width and under a
          // coarse pointer an invisible margin widens its tap area to 40px.
          <Button
            variant="ghost"
            size={cover ? "icon" : "icon-sm"}
            aria-label="Close"
            disabled={busy}
            focusableWhenDisabled
            onClick={() => leave("close-control")}
            className={cn(
              INERT_BUTTON,
              cover
                ? "size-10 shrink-0"
                : "absolute top-2 right-2 after:absolute after:-inset-[7px] after:hidden max-md:after:block pointer-coarse:after:block",
            )}
          >
            <X />
          </Button>
        ) : null}
        {cover ? null : description}
      </div>
      {hasSearch ? (
        <div className={cover ? "shrink-0 border-b px-4 py-3" : "shrink-0 px-4 pt-4 pb-3"}>
          <SearchField search={props.search!} inputRef={searchRef} />
        </div>
      ) : null}
      {frame?.toolbar ? (
        <div
          data-slot="modal-toolbar"
          className={cover ? "shrink-0 border-b px-4 py-2" : "shrink-0 px-4 pb-3"}
        >
          {frame.toolbar}
        </div>
      ) : null}
      {!cover && !hasBody ? null : steady ? (
        <ScrollArea className={cover ? "min-h-0 flex-1" : "h-72 min-h-0 shrink border-t"}>
          <div ref={body} data-slot="modal-body" className={bodyClass}>
            {bodyDescription}
            {props.children}
          </div>
        </ScrollArea>
      ) : (
        <div
          ref={body}
          data-slot="modal-body"
          className={cn(
            "min-h-0 overflow-y-auto",
            cover ? "flex-1 overscroll-contain" : fixedHeight ? "flex-1" : "shrink",
            // -mt-1 with the padding's pt-1: room for a focus ring on the
            // body's first control, which the scroller would otherwise cut.
            !cover && kind !== "picker" && "-mt-1",
            bodyClass,
          )}
        >
          {bodyDescription}
          {props.children}
        </div>
      )}
      {kind === "picker" ? (
        props.doors ? (
          <ModalDoors doors={props.doors} phone={phone} cover={cover} />
        ) : null
      ) : (
        <ModalFooter
          stacked={phone}
          cover={cover}
          safe={safe}
          safeRef={callerSafeRef}
          safeEnds={kind === "report"}
          leading={leading}
          actions={actions.filter((a) => !leading.includes(a))}
          note={kind === "confirm" ? undefined : props.footerNote}
        />
      )}
    </>
  )
}

function SearchField({
  search,
  inputRef,
}: {
  search: ModalSearch
  inputRef: Ref<HTMLInputElement>
}): ReactNode {
  return (
    <div className="flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50 max-md:min-h-10 pointer-coarse:min-h-10">
      <Search className="size-4 shrink-0 text-muted-foreground" />
      <input
        ref={inputRef}
        value={search.value}
        onChange={(event) => search.onChange(event.target.value)}
        placeholder={search.label}
        aria-label={search.label}
        {...search.fieldProps}
        className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
      />
    </div>
  )
}
