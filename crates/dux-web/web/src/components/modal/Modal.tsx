import { ChevronLeft, Search, X } from "lucide-react"
import { useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from "react"

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

import { closeReasonFrom } from "./closeReason"
import { focusTarget, isTextEntry } from "./focus"
import {
  CARD_ANCHOR,
  CARD_HEIGHT,
  CARD_WIDTH,
  COVER_BOTTOM_INSET,
  FIELD_TEXT_SIZE,
  INERT_BUTTON,
  modalPresentation,
} from "./layout"
import { ModalDoors, ModalFooter, type SafeButton } from "./ModalFooter"
import { RowReasonContext } from "./rowReason"
import type { ModalAction, ModalCloseReason, ModalProps, ModalRow, ModalSearch } from "./types"

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
  // A Back that arrived while busy, answered once the modal is free: a way out,
  // or with no history entry of its own the step's Back.
  const backWaits = useRef<"leave" | "step" | null>(null)
  // Why the row the modal's action changes may not be changed from here.
  const [rowReason, setRowReason] = useState<string | null>(null)
  // The last control focused in the form outside its footer, where Keep
  // editing gives focus back: the Cancel that raised the question is not where
  // the person was.
  const editing = useRef<HTMLElement | null>(null)

  /** Takes a way out, and says whether the modal is closing. */
  const leave = (reason: ModalCloseReason): boolean => {
    if (busy) {
      if (reason === "back") backWaits.current = "leave"
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
    if (!open) backWaits.current = null
    const waiting = backWaits.current
    if (!open || busy || waiting === null) return
    backWaits.current = null
    if (waiting === "leave") leave("back")
    else if (props.kind !== "confirm") props.back?.onSelect()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, busy])

  const presentation = modalPresentation(kind, phone)
  const frame = kind === "confirm" ? null : props
  const heldReason = rowReason ?? props.writeBlocked ?? null
  const parts = withRowReason(props, heldReason)
  const useReason = useContext(RowReasonContext)
  return (
    <>
      {props.row && useReason ? (
        <RowChangeReason row={props.row} useReason={useReason} onReason={setRowReason} />
      ) : null}
      <Dialog
        open={open}
        onOpenChange={(next, details) => {
          if (next) return
          const reason = closeReasonFrom(details.reason)
          // A close no person asked for is not a way out: `open` is the caller's.
          if (reason === null || !leave(reason)) details.cancel()
        }}
        onBack={() => !leave("back")}
        // With no entry of its own, Back is taken only as the step's own Back.
        onBackStep={() => {
          const back = frame?.back
          if (!back) return false
          if (busy) backWaits.current = "step"
          else back.onSelect()
          return true
        }}
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
          // A field's own Enter (or a text area's Ctrl/Cmd-Enter) runs the
          // caller's submit, which the footer's gate never sees: while the
          // modal or its submit is busy, or its change may not be made from
          // here, it runs nothing.
          onKeyDownCapture={(event) => {
            // An Enter that ends an IME composition is the composition's: it
            // reaches no caller's handler, and its default stays so it commits.
            if (event.key === "Enter" && (event.nativeEvent.isComposing || event.keyCode === 229)) {
              event.stopPropagation()
              return
            }
            const held = busy || heldReason !== null || (kind === "form" && props.submit.busy === true)
            if (!held) return
            if (event.key !== "Enter" || !submitsOnEnter(event.target, event)) return
            if (event.target.closest('[data-slot="dialog-content"]') !== event.currentTarget) return
            event.preventDefault()
            event.stopPropagation()
          }}
          onFocus={(event) => {
            const target = event.target
            if (target.closest('[data-slot="modal-footer"]') === null) editing.current = target
          }}
          className={cn(
            FIELD_TEXT_SIZE,
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
            {...parts}
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
          finalFocus={() => (editing.current?.isConnected ? editing.current : true)}
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

// Whether Enter on this element can be a submit: in a field, or with Ctrl or Cmd
// in a text area, where a plain Enter is a line break.
function submitsOnEnter(
  target: EventTarget,
  keys: { ctrlKey: boolean; metaKey: boolean },
): target is HTMLElement {
  if (target instanceof HTMLTextAreaElement) return keys.ctrlKey || keys.metaKey
  return target instanceof HTMLInputElement
}

// Asks the row's answer in a component of its own, mounted only for a modal
// that names a row, so a modal that names none asks nothing.
function RowChangeReason({
  row,
  useReason,
  onReason,
}: {
  row: ModalRow
  useReason: (row: ModalRow) => string | null
  onReason: (reason: string | null) => void
}): null {
  const reason = useReason(row)
  useLayoutEffect(() => onReason(reason), [reason, onReason])
  useLayoutEffect(() => () => onReason(null), [onReason])
  return null
}

// The modal's own action, disabled with the reason its change may not be made
// from here while it has one.
function withRowReason(props: ModalProps, reason: string | null): ModalProps {
  if (reason === null) return props
  switch (props.kind) {
    case "confirm":
      return { ...props, confirm: { ...props.confirm, disabledReason: reason } }
    case "form":
      return { ...props, submit: { ...props.submit, disabledReason: reason } }
    case "picker":
    case "report":
      return props
  }
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
  // A computer's picker, and a report that asks for it, close from the corner.
  const corner = !cover && (kind === "picker" || (kind === "report" && props.closeAt === "corner"))
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
      closeControl: popup.querySelector<HTMLElement>("[data-modal-close]"),
    })
  }
  useLayoutEffect(() => {
    onFocusTarget(target)
  })
  const last = useRef<{ open: boolean; step: string | undefined } | null>(null)
  // The card's height as last painted, read when the step changes.
  const shownHeight = useRef(0)
  useLayoutEffect(() => {
    const was = last.current
    last.current = { open, step }
    const popup = header.current?.closest('[data-slot="dialog-content"]')
    if (!open) {
      if (popup instanceof HTMLElement) popup.style.height = ""
      return
    }
    const stepChanged = was !== null && was.open && was.step !== step
    // On open the popup places focus itself, unless a control inside already
    // took it; that case and a change of step are placed here.
    // A card keeps the height it had across a change of step, in both
    // directions, so the list or the button under the pointer does not jump; a
    // step with more to show scrolls inside it. The card's max-height still
    // caps it to the window, and a resize of the window lets go of the held
    // height so the card is measured again for the step it shows.
    if (stepChanged && !cover && popup instanceof HTMLElement && shownHeight.current > 0) {
      popup.style.height = `${shownHeight.current}px`
    }
    if (!stepChanged && !popup?.contains(document.activeElement)) return
    target()?.focus({ preventScroll: true })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, step])
  useLayoutEffect(() => {
    const popup = header.current?.closest<HTMLElement>('[data-slot="dialog-content"]')
    if (!popup) return
    if (cover) popup.style.height = ""
    shownHeight.current = cover ? 0 : popup.offsetHeight
  })
  useEffect(() => {
    if (!open || cover) return
    const release = () => {
      const popup = header.current?.closest<HTMLElement>('[data-slot="dialog-content"]')
      if (!popup) return
      popup.style.height = ""
      shownHeight.current = popup.offsetHeight
    }
    window.addEventListener("resize", release)
    return () => window.removeEventListener("resize", release)
  }, [open, cover])

  // The body's scroll position, kept across a switch between card and cover.
  // The browser clamps it wherever the other presentation has less to scroll
  // (a tall cover often has nothing to), and the switch back would otherwise
  // start at the top. The scroll events that clamping fires arrive frames
  // later and are not the person's, so after a switch none is kept until the
  // person next wheels, touches, presses or types.
  const keptScroll = useRef(0)
  const switching = useRef(false)
  const wasCover = useRef(cover)
  useEffect(() => {
    const scroller = scrollerOf(body.current)
    if (!open || scroller === null) return
    const onScroll = () => {
      if (!switching.current) keptScroll.current = scroller.scrollTop
    }
    const onInput = () => {
      switching.current = false
    }
    const inputs = ["wheel", "touchstart", "pointerdown", "keydown"] as const
    scroller.addEventListener("scroll", onScroll)
    for (const type of inputs) window.addEventListener(type, onInput, true)
    return () => {
      scroller.removeEventListener("scroll", onScroll)
      for (const type of inputs) window.removeEventListener(type, onInput, true)
    }
  }, [open, cover])
  useLayoutEffect(() => {
    if (wasCover.current === cover) return
    wasCover.current = cover
    const scroller = scrollerOf(body.current)
    if (scroller === null) return
    switching.current = true
    scroller.scrollTop = keptScroll.current
  }, [cover])

  // In a cover, a field that takes focus is brought into the part of the body
  // on screen, and brought back whenever the cover shrinks under it: the soft
  // keyboard arrives after the focus, and the cover is what it shrinks.
  useEffect(() => {
    const scroller = body.current
    const popup = header.current?.closest<HTMLElement>('[data-slot="dialog-content"]')
    if (!open || !cover || scroller === null || !popup) return
    const reveal = () => {
      const field = document.activeElement
      if (field instanceof HTMLElement && scroller.contains(field) && isTextEntry(field)) {
        field.scrollIntoView?.({ block: "nearest" })
      }
    }
    scroller.addEventListener("focusin", reveal)
    const shrunk = typeof ResizeObserver === "function" ? new ResizeObserver(reveal) : null
    shrunk?.observe(popup)
    return () => {
      scroller.removeEventListener("focusin", reveal)
      shrunk?.disconnect()
    }
  }, [open, cover])

  // A report's Close is drawn on a card only, since a cover's title row closes
  // it; a form keeps Cancel on both, because Cancel abandons what was typed.
  const safeLabel =
    kind === "confirm"
      ? props.cancelLabel
      : kind === "form"
        ? (props.cancelLabel ?? "Cancel")
        : kind === "report" && !cover && !corner
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
  // The description says what the modal is for, so it comes before a search or
  // a toolbar on both presentations. A cover with neither lets it scroll with
  // the body, under the title row; with one, it stays above it.
  const coverAbove = cover && (hasSearch || Boolean(frame?.toolbar))
  const bodyDescription = !cover || coverAbove ? null : kind === "picker" && description ? (
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
        data-toast-clear={cover ? "" : undefined}
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
            ref={frame?.backRef}
            variant="ghost"
            size={cover ? "icon" : "sm"}
            aria-label={`Back to ${back.label}`}
            disabled={busy}
            focusableWhenDisabled
            onClick={back.onSelect}
            className={cn(
              INERT_BUTTON,
              cover
                ? "-ml-2 size-10 shrink-0"
                : "-mt-1 -ml-1.5 h-8 self-start px-1.5 text-muted-foreground pointer-coarse:min-h-10",
            )}
          >
            <ChevronLeft />
            {cover ? null : back.label}
          </Button>
        ) : null}
        <DialogTitle
          className={cn("leading-snug", cover ? "min-w-0 flex-1" : corner && "pr-8")}
        >
          {props.title}
        </DialogTitle>
        {cover || corner ? (
          // The card's 28px box keeps its place; at phone width and under a
          // coarse pointer an invisible margin widens its tap area to 40px.
          <Button
            variant="ghost"
            size={cover ? "icon" : "icon-sm"}
            aria-label="Close"
            disabled={busy}
            focusableWhenDisabled
            data-modal-close=""
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
      {coverAbove && description ? <div className="shrink-0 px-4 pt-3">{description}</div> : null}
      {hasSearch ? (
        <div
          className={
            cover
              ? cn("shrink-0 border-b px-4 pb-3", description ? "pt-2" : "pt-3")
              : "shrink-0 px-4 pt-4 pb-3"
          }
        >
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
            // `grow` fills a height kept from an earlier step; with none kept
            // there is nothing to grow into.
            cover ? "flex-1 overscroll-contain" : fixedHeight ? "flex-1" : "shrink grow",
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
          busy={busy}
          note={kind === "confirm" ? undefined : props.footerNote}
        />
      )}
    </>
  )
}

// What scrolls a modal's body: the list's scroll area where it has one.
function scrollerOf(body: HTMLElement | null): HTMLElement | null {
  if (body === null) return null
  return body.closest<HTMLElement>('[data-slot="scroll-area-viewport"]') ?? body
}

function SearchField({
  search,
  inputRef,
}: {
  search: ModalSearch
  inputRef: Ref<HTMLInputElement>
}): ReactNode {
  return (
    // A label, so a press anywhere in the 40px box lands in the field.
    <label className="flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50 max-md:min-h-10 pointer-coarse:min-h-10">
      <Search className="size-4 shrink-0 text-muted-foreground" />
      {/* Plain text, never `type="search"`: that clears itself on Escape,
          which here also closes the modal, and Chromium draws a clear button
          of its own in it. */}
      <input
        ref={inputRef}
        type="text"
        value={search.value}
        onChange={(event) => search.onChange(event.target.value)}
        placeholder={search.label}
        aria-label={search.label}
        {...search.fieldProps}
        data-modal-search=""
        className="min-w-0 flex-1 self-stretch bg-transparent py-2 text-base outline-none placeholder:text-muted-foreground md:text-sm"
      />
    </label>
  )
}
