import { useEffect, useRef, useState } from "react"
import type * as React from "react"
import { SquarePen, SquareSlash } from "lucide-react"
import { Popover as PopoverPrimitive } from "@base-ui/react/popover"

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { Button } from "@/components/ui/button"
import { MatchHighlight } from "@/components/MatchHighlight"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { targetPtyId } from "@/components/terminalAreaModel"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useMacrosBlockedHint } from "@/hooks/use-macros-blocked"
import { useIsMobile } from "@/hooks/use-mobile"
import { FOCUS_RING } from "@/lib/focusRing"
import { macrosForTarget } from "@/lib/macros"
import { cmdkMatchRanges } from "@/lib/searchMatch"
import { usePaneMacroTarget } from "@/lib/paneMacroTarget"
import { cn } from "@/lib/utils"
import { openMacrosDialog, runMacro, useDux } from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import type { MacroView } from "@/lib/types"

// A quick-picker for sending a text macro to the pane it belongs to: a
// filterable list restricted to that target's surface, run by Enter or click.
// There is deliberately no confirmation and no status; `runMacro` writes the
// payload down that pane's own socket, or splices it into its compose draft
// while that is the typing surface, and the feedback is the text appearing.
//
// While the take-over card covers that pane the trigger is disabled: this tab is
// not driving the pty, and a macro is not a way to start. So is it while the
// tab is not running, when there is no pane at all to carry the macro.
//
// The trigger is rendered as an absolutely-positioned sibling of the xterm
// host, never inside the unpadded container xterm opens into, so it cannot
// change the terminal's box measurement.
//
// With `anchored` there is no trigger at all: the picker is opened from a menu
// row and placed against that menu's trigger, for the computer header whose
// labelled trigger has folded into its `⋯`.
export function MacroPopover({
  target,
  finalFocus,
  variant = "labeled",
  anchored,
}: {
  target: SelectedTarget
  // Where focus lands when the popover closes (selecting a macro, Esc, or
  // dismissing). TerminalPane points this at the xterm helper textarea so the
  // cursor returns to the terminal rather than the "Macros…" trigger button.
  // That is the whole point of the feature: running a macro pastes its text into
  // the agent's input WITHOUT submitting, so focus must be on the terminal for
  // the user to review and press Enter to submit. With the default
  // trigger-return, Enter would just re-press this trigger and re-open the menu.
  // This intentionally overrides the usual "return focus to the trigger" popover
  // convention because the trigger floats over a live terminal the user drives.
  finalFocus?: () => HTMLElement | null
  // "labeled" is the desktop trigger; "pill" is the round ghost button of the
  // phone flap and the theater pill, since a square outline button inside that
  // one rounded control would read as a second surface. The pill passes no
  // finalFocus, so a dismissal returns focus to the trigger; where a pick lands
  // is in `resolveFinalFocus`.
  variant?: "labeled" | "pill"
  // The triggerless picker: open while `open` says so, placed against `ref`,
  // which is also where focus goes back when no pane can take it. It closes as
  // the labelled trigger's picker does; `variant` is not read.
  anchored?: {
    ref: React.RefObject<HTMLElement | null>
    open: boolean
    onOpenChange: (open: boolean) => void
  }
}) {
  const { bootstrap } = useDux()
  const blocked = useMacrosBlockedHint(target)
  const [ownOpen, setOwnOpen] = useState(false)
  const open = anchored ? anchored.open : ownOpen
  const setOpen = anchored ? anchored.onOpenChange : setOwnOpen
  // A pane that becomes blocked closes the picker, so it does not come back by
  // itself once the pane is free again.
  useEffect(() => {
    if (blocked && open) setOpen(false)
  }, [blocked, open, setOpen])
  const isMobile = useIsMobile()
  const coarsePointer = useIsCoarsePointer()
  // Set when the LAST pick landed in the mobile compose draft rather than the
  // PTY, consumed by the close-focus resolver below. A ref, not state: it is
  // read during Base UI's close-focus pass, never rendered.
  const pickedIntoComposeRef = useRef(false)
  // Set when the LAST pick was written straight to the pty, read the same way.
  const pickedIntoPtyRef = useRef(false)

  const allMacros = bootstrap?.macros ?? []
  const macros = macrosForTarget(allMacros, target)
  const pane = usePaneMacroTarget(targetPtyId(target))
  const labeled = anchored !== undefined || variant === "labeled"

  function handleRun(name: string) {
    // The payload goes to this target's pane, or into its compose draft while
    // that bar is the typing surface. The returned destination says which
    // happened, which steers the close focus below.
    const destination = runMacro(name, target)
    pickedIntoComposeRef.current = destination === "compose"
    pickedIntoPtyRef.current = destination === "pty"
    setOpen(false)
  }

  // Where focus lands when the popover closes: into the compose draft for a
  // pick that landed there, because Base UI owns focus during a close and would
  // otherwise yank the keyboard back to the trigger. Otherwise the caller's
  // finalFocus, else this target's pane's typing surface for the desktop
  // labelled trigger (and the anchored picker standing in for it) on any close
  // and for the pill after a pick sent to the pty, else Base UI's
  // return-to-trigger, which for the anchored picker is its anchor.
  //
  // Every trigger sits outside `TerminalPane` and has no ref to hand in, so it
  // reads the surface the pane published under its own pty id. The pill skips
  // that on a coarse pointer, where focusing a terminal textarea pops a soft
  // keyboard nobody asked for.
  function resolveFinalFocus(): HTMLElement | boolean | null {
    const pickedIntoPty = pickedIntoPtyRef.current
    pickedIntoPtyRef.current = false
    if (pickedIntoComposeRef.current) {
      pickedIntoComposeRef.current = false
      const composeTarget = pane?.compose?.target() ?? null
      if (composeTarget) return composeTarget
    }
    if (finalFocus) return finalFocus()
    const back = anchored?.ref.current ?? true
    if (labeled || (pickedIntoPty && !coarsePointer)) {
      return pane?.typingSurface() ?? back
    }
    return back
  }

  if (blocked) {
    // The anchored picker's menu row is disabled with the same reason.
    return anchored ? null : <DisabledTrigger variant={variant} hint={blocked} />
  }

  return (
    // Tab stays in the picker, which base-ui does only with a Close part inside
    // it; the page keeps its scroll and a click outside still dismisses it.
    <Popover open={open} onOpenChange={setOpen} modal="trap-focus">
      {/* The ellipsis on the label says the button opens a menu of choices
          rather than acting.

          The labeled variant is `outline` to match the desktop header's
          controls, at their `h-8` token (40px under a finger), so the label
          changes the width and nothing else. The pill variant is not, because
          it lives inside the floating theater pill, one rounded surface that
          would read as two with a bordered button in it.

          The shape lives here rather than at the call site so the live and the
          disabled trigger cannot drift apart. */}
      {anchored ? null : (
        <PopoverTrigger render={<TriggerButton shape={variant} />}>
          <TriggerFace variant={variant} />
        </PopoverTrigger>
      )}
      <PopoverContent
        anchor={anchored?.ref}
        align="end"
        // Desktop: the fixed 288px-wide anchored panel. Phone: PopoverContent
        // presents as the shared bottom sheet (full width, so no w-72), and
        // the sheet's own whole-popup scroll is replaced with a flex column so
        // the search field and the Edit-macros footer stay pinned while the
        // LIST scrolls, same as the desktop layout. p-0 eats the sheet's
        // safe-area bottom padding along with the rest (tailwind-merge folds
        // the whole padding group), so the pb is restated: it is what keeps
        // the Edit-macros footer above a phone's home-indicator strip.
        className={
          isMobile
            ? "flex flex-col overflow-hidden p-0 pb-[max(env(safe-area-inset-bottom),0.25rem)]"
            : "w-72 p-0"
        }
        finalFocus={resolveFinalFocus}
      >
        {allMacros.length === 0 ? (
          <div className="px-3 py-6 text-center text-sm text-muted-foreground">
            No macros found. Start by creating one!
          </div>
        ) : macros.length === 0 ? (
          <div className="px-3 py-6 text-center text-sm text-muted-foreground">
            No macros for this target kind. Add one via Edit macros below.
          </div>
        ) : (
          <MacroCommand macros={macros} autoFocus={!isMobile} onRun={handleRun} />
        )}
        {/* The picker's Close part: it closes the picker on its way to the
            dialog, so it is the one control that can carry the role. */}
        <PopoverPrimitive.Close
          // shrink-0: the footer never gives up height to the list; when the
          // popup is at its viewport cap it is the list that scrolls.
          className="flex w-full shrink-0 items-center gap-2 border-t px-3 py-2 text-left text-sm text-muted-foreground hover:text-foreground max-md:min-h-10 pointer-coarse:min-h-10"
          onClick={() => {
            setOpen(false)
            openMacrosDialog()
          }}
        >
          <SquarePen className="size-3.5 shrink-0" />
          Edit macros…
        </PopoverPrimitive.Close>
      </PopoverContent>
    </Popover>
  )
}

// The macro list and its search. Mounted inside the popup, so the search starts
// empty on every open. The search is held here rather than left to cmdk so a
// row can light the letters cmdk's loose match found; cmdk still does the
// filtering, against the row's value, which is the name it shows.
function MacroCommand({
  macros,
  autoFocus,
  onRun,
}: {
  macros: MacroView[]
  autoFocus: boolean
  onRun: (name: string) => void
}) {
  const [search, setSearch] = useState("")
  return (
    // min-h-0 lets the Command column shrink when the popup hits its
    // viewport cap, so the list below scrolls instead of pushing the
    // Edit-macros footer off screen.
    <Command className="min-h-0">
      {/* No autofocus in the sheet: menus never pop the soft keyboard
          on open, and this now presents as one of them; tapping the
          field summons the keyboard. Desktop keeps type-immediately
          (the filters-are-type-immediately convention). */}
      <CommandInput
        placeholder="Search macros…"
        autoFocus={autoFocus}
        value={search}
        onValueChange={setSearch}
      />
      {/* CommandGroup's padding puts breathing room between the search
          field and the first result, matching the gap above the Edit
          macros footer below. */}
      {/* max-h-[none]: the primitive's fixed cap would hold this list
          at 288px with a whole screen free below it. Like every other
          menu, the bound is the popup's max-h-(--available-height) (see
          popover.tsx); min-h-0 makes the list the flex child that
          shrinks, which is what engages its own overflow-y scroll. The
          arbitrary-value form is deliberate: the installed
          tailwind-merge does not recognize bare max-h-none as a max-h
          conflict, so the primitive's max-h-72 would survive beside it
          and which one wins would be stylesheet-order luck. */}
      <CommandList className="max-h-[none] min-h-0">
        <CommandEmpty>No matching macros.</CommandEmpty>
        <CommandGroup>
          {macros.map((macro) => (
            <CommandItem
              key={macro.name}
              value={macro.name}
              className="cursor-pointer"
              onSelect={() => onRun(macro.name)}
            >
              <span>
                <MatchHighlight
                  text={macro.name}
                  ranges={cmdkMatchRanges(macro.name, search)}
                />
              </span>
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
    </Command>
  )
}

type TriggerVariant = "labeled" | "pill"

// The trigger's one shape, shared by the live and the disabled control so the
// cluster does not move when the take-over card comes or goes.
function TriggerButton({
  shape,
  className,
  ...props
}: { shape: TriggerVariant } & Omit<
  React.ComponentProps<typeof Button>,
  "variant" | "size"
>) {
  if (shape === "pill") {
    return (
      <Button
        variant="ghost"
        size="icon"
        aria-label="Run a macro"
        {...props}
        className={cn("size-10 shrink-0 rounded-full", className)}
      />
    )
  }
  return (
    <Button
      variant="outline"
      aria-label="Run a macro"
      {...props}
      className={cn("pointer-coarse:h-10", className)}
    />
  )
}

function TriggerFace({ variant }: { variant: TriggerVariant }) {
  return (
    <>
      <SquareSlash />
      {variant === "labeled" ? <>Macros…</> : null}
    </>
  )
}

// A disabled button takes no pointer events, so the hint hangs off a focusable
// wrapper around it, through the shared tooltip; the wrapper wears the focus
// ring the disabled button cannot. The button keeps its full size, so the touch
// floor and the cluster's height token hold while it is disabled.
function DisabledTrigger({
  variant,
  hint,
}: {
  variant: TriggerVariant
  hint: string
}) {
  return (
    <SimpleTooltip content={hint}>
      <span
        tabIndex={0}
        data-testid="macros-disabled"
        className={cn(
          "inline-flex shrink-0",
          variant === "pill" ? "rounded-full" : "rounded-lg",
          FOCUS_RING,
        )}
      >
        <TriggerButton shape={variant} disabled>
          <TriggerFace variant={variant} />
        </TriggerButton>
      </span>
    </SimpleTooltip>
  )
}
