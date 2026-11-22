import { ChevronLeft, Search, X } from "lucide-react"
import { useLayoutEffect, useRef, useState, type ReactNode, type Ref } from "react"

import { ConfirmDiscardEditsDialog } from "@/components/ConfirmDiscardEditsDialog"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useIsMobile } from "@/hooks/use-mobile"
import { cn } from "@/lib/utils"

import { focusTarget } from "./focus"
import { CARD_WIDTH, COVER_BOTTOM_INSET, modalPresentation } from "./layout"
import { ModalDoors, ModalFooter, type SafeButton } from "./ModalFooter"
import type { ModalAction, ModalProps, ModalSearch } from "./types"

/** Every modal in the web UI. The caller hands over its kind and its parts;
 * the kind decides how it is presented, focused, laid out and closed. */
export function Modal(props: ModalProps) {
  const phone = useIsMobile()
  const { open, onClose, kind } = props
  const unsaved = kind === "form" ? (props.unsavedEdits ?? null) : null
  // Why the discard confirm is up: a press here, or the browser's Back.
  const [asked, setAsked] = useState<"press" | "back" | null>(null)
  const asking = open ? asked : null
  const placeFocus = useRef<(() => HTMLElement | null) | null>(null)

  const leave = (cancel?: () => void, byBack = false) => {
    if (props.busy === true) return cancel?.()
    if (unsaved !== null && unsaved.changes > 0) {
      cancel?.()
      setAsked((was) => was ?? (byBack ? "back" : "press"))
      return
    }
    setAsked(null)
    onClose()
  }

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(next, details) => {
          if (!next) leave(details.cancel, details.reason === "imperative-action")
        }}
        // A Back that was answered with a question is spent: giving the entry
        // up stops Back's repeated asking, and Keep editing takes a new one.
        historyEntry={props.historyEntry !== false && asking !== "back"}
      >
        <DialogContent
          cover={modalPresentation(kind, phone) === "cover"}
          data-kind={kind}
          data-presentation={modalPresentation(kind, phone)}
          showCloseButton={!phone && kind === "picker"}
          destructive={kind === "confirm" && props.confirm.tone === "destructive"}
          finalFocus={props.finalFocus}
          initialFocus={() => placeFocus.current?.() ?? true}
          className={cn(
            modalPresentation(kind, phone) === "card" &&
              cn("flex flex-col gap-0 p-0", CARD_WIDTH[props.size ?? "sm"]),
          )}
        >
          <ModalParts
            {...props}
            phone={phone}
            leave={() => leave()}
            onFocusTarget={(target) => {
              placeFocus.current = target
            }}
          />
        </DialogContent>
      </Dialog>
      {unsaved !== null ? (
        <ConfirmDiscardEditsDialog
          open={asking !== null}
          holder={unsaved.holder}
          changes={unsaved.changes}
          onKeep={() => setAsked(null)}
          onDiscard={() => {
            setAsked(null)
            onClose()
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
function ModalParts(
  props: ModalProps & {
    phone: boolean
    leave: () => void
    /** Handed the function that says what takes focus now, on every render. */
    onFocusTarget: (target: () => HTMLElement | null) => void
  },
) {
  const { kind, phone, leave, open, onFocusTarget } = props
  const cover = modalPresentation(kind, phone) === "cover"
  const header = useRef<HTMLDivElement | null>(null)
  const body = useRef<HTMLDivElement | null>(null)
  const safeRef = useRef<HTMLButtonElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)

  const back = kind === "confirm" ? undefined : props.back
  const step = kind === "confirm" ? undefined : props.step
  const named = kind === "confirm" ? undefined : props.initialFocus
  const target = () => {
    const popup = header.current?.closest<HTMLElement>('[data-slot="dialog-content"]')
    if (!popup) return null
    return focusTarget({
      kind,
      phone,
      popup,
      body: body.current,
      named: named?.current ?? null,
      safeButton: safeRef.current,
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
  const safe: SafeButton | null =
    kind === "confirm"
      ? { label: props.cancelLabel, onSelect: leave, disabled: props.busy === true }
      : kind === "form"
        ? {
            label: props.cancelLabel ?? "Cancel",
            onSelect: leave,
            disabled: props.busy === true,
          }
        : kind === "report" && !cover
          ? { label: "Close", onSelect: leave, disabled: props.busy === true }
          : null

  const search =
    kind === "picker" && props.search ? (
      <SearchField search={props.search} inputRef={searchRef} />
    ) : null
  const description = props.description ? (
    <DialogDescription className="text-pretty">{props.description}</DialogDescription>
  ) : null
  const footer =
    kind === "picker" ? (
      props.doors ? (
        <ModalDoors doors={props.doors} phone={phone} cover={cover} />
      ) : null
    ) : (
      <ModalFooter
        stacked={phone}
        cover={cover}
        safe={safe}
        safeRef={safeRef}
        actions={footerActions(props)}
        note={kind === "confirm" ? undefined : props.footerNote}
      />
    )
  const hasFooter =
    kind === "picker" ? props.doors !== undefined : safe !== null || footerActions(props).length > 0

  if (cover) {
    return (
      <>
        <div
          ref={header}
          data-slot="modal-header"
          className="flex min-h-14 shrink-0 items-center gap-1 border-b py-2 pr-2 pl-4"
        >
          {back ? (
            <Button
              variant="ghost"
              size="icon"
              className="-ml-2 size-10 shrink-0"
              aria-label={back.label}
              onClick={back.onSelect}
            >
              <ChevronLeft />
            </Button>
          ) : null}
          <DialogTitle className="min-w-0 flex-1 leading-snug">{props.title}</DialogTitle>
          <DialogClose
            render={
              <Button variant="ghost" size="icon" className="size-10 shrink-0" aria-label="Close" />
            }
          >
            <X />
          </DialogClose>
        </div>
        {search ? <div className="shrink-0 border-b px-4 py-3">{search}</div> : null}
        <div
          ref={body}
          data-slot="modal-body"
          className={cn(
            "min-h-0 flex-1 overflow-y-auto overscroll-contain",
            kind === "picker" ? "p-2" : "flex flex-col gap-4 p-4",
            !hasFooter && COVER_BOTTOM_INSET,
          )}
        >
          {description && kind === "picker" ? (
            <div className="px-2 pt-1 pb-2">{description}</div>
          ) : (
            description
          )}
          {props.children}
        </div>
        {footer}
      </>
    )
  }

  const hasBody = props.children !== undefined && props.children !== null && props.children !== false
  return (
    <>
      <div
        ref={header}
        data-slot="modal-header"
        className={cn(
          "flex shrink-0 flex-col gap-2 p-4",
          kind === "picker" ? "pb-3" : !hasBody && "pb-6",
        )}
      >
        {back ? (
          // Quieter than the modal's actions: it navigates. 32px, as its only
          // neighbour is the corner close across the header.
          <Button
            variant="ghost"
            size="sm"
            onClick={back.onSelect}
            className="-mt-1 -ml-1.5 h-8 self-start px-1.5 text-muted-foreground pointer-coarse:min-h-10"
          >
            <ChevronLeft />
            {back.label}
          </Button>
        ) : null}
        <DialogTitle className={cn("leading-snug", kind === "picker" && "pr-8")}>
          {props.title}
        </DialogTitle>
        {description}
        {search ? <div className="mt-2">{search}</div> : null}
      </div>
      {hasBody ? (
        <CardBody
          padded={kind !== "picker"}
          steady={kind === "picker" && props.steadyHeight === true}
          bodyRef={body}
        >
          {props.children}
        </CardBody>
      ) : null}
      {footer}
    </>
  )
}

function CardBody({
  padded,
  steady,
  bodyRef,
  children,
}: {
  padded: boolean
  steady: boolean
  bodyRef: Ref<HTMLDivElement>
  children: ReactNode
}) {
  if (padded) {
    return (
      // -mt-1 pt-1: room for a focus ring on the body's first control, which
      // the scroller would otherwise cut.
      <div
        ref={bodyRef}
        data-slot="modal-body"
        className="-mt-1 flex min-h-0 shrink flex-col gap-4 overflow-y-auto px-4 pt-1 pb-6"
      >
        {children}
      </div>
    )
  }
  if (steady) {
    return (
      <ScrollArea className="h-72 min-h-0 shrink border-t">
        <div ref={bodyRef} data-slot="modal-body" className="p-2">
          {children}
        </div>
      </ScrollArea>
    )
  }
  return (
    <div
      ref={bodyRef}
      data-slot="modal-body"
      className="min-h-0 shrink overflow-y-auto p-2"
    >
      {children}
    </div>
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
        type="search"
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
