import { useRef, useState } from "react"
import type * as React from "react"
import { SquarePen, SquareSlash } from "lucide-react"

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
import { targetPtyId, targetTabDormant } from "@/components/terminalAreaModel"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { useIsMobile } from "@/hooks/use-mobile"
import { FOCUS_RING } from "@/lib/focusRing"
import { macrosForTarget } from "@/lib/macros"
import { subsequenceMatchRanges } from "@/lib/searchMatch"
import { usePaneMacroTarget } from "@/lib/paneMacroTarget"
import { cn } from "@/lib/utils"
import { openMacrosDialog, runMacro, useDux, useDuxSelector } from "@/lib/store"
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
export function MacroPopover({
  target,
  finalFocus,
  variant = "labeled",
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
  // "labeled" is the desktop trigger; "icon" is the phone header's icon button;
  // "pill" is the round ghost button of the phone flap and the theater pill,
  // since a square outline button inside that one rounded control would read
  // as a second surface. The last two pass no finalFocus, so a dismissal
  // returns focus to the trigger; where a pick lands is in `resolveFinalFocus`.
  variant?: "labeled" | "icon" | "pill"
}) {
  const { bootstrap } = useDux()
  const dormant = useDuxSelector((s) => targetTabDormant(target, s))
  const [open, setOpen] = useState(false)
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
  // labelled trigger on any close and for every other variant after a pick
  // sent to the pty, else Base UI's return-to-trigger.
  //
  // Every trigger sits outside `TerminalPane` and has no ref to hand in, so it
  // reads the surface the pane published under its own pty id. The icon and
  // pill variants skip that on a coarse pointer, where focusing a terminal
  // textarea pops a soft keyboard nobody asked for.
  function resolveFinalFocus(): HTMLElement | boolean | null {
    const pickedIntoPty = pickedIntoPtyRef.current
    pickedIntoPtyRef.current = false
    if (pickedIntoComposeRef.current) {
      pickedIntoComposeRef.current = false
      const composeTarget = pane?.compose?.target() ?? null
      if (composeTarget) return composeTarget
    }
    if (finalFocus) return finalFocus()
    if (variant === "labeled" || (pickedIntoPty && !coarsePointer)) {
      return pane?.typingSurface() ?? true
    }
    return true
  }

  if (dormant) {
    return <DisabledTrigger variant={variant} hint="Start the tab to send macros" />
  }
  if (pane?.takeoverCard) {
    return <DisabledTrigger variant={variant} hint="Take over to send macros" />
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      {/* The ellipsis on the label says the button opens a menu of choices
          rather than acting. The icon variant drops the label because it sits
          among icon-only controls in the phone header; do not give it one back.

          The labeled and icon variants are `outline` to match the desktop
          header's controls. The pill variant is not, because it lives inside
          the floating theater pill, one rounded surface that would read as two
          with a bordered button in it. Its height is the button's default `h-8`
          token, the same as the `size="icon"` buttons beside it, so the label
          changes the width and nothing else.

          The shape lives here rather than at the call site so the trigger
          cannot drift from the `±N` and `⋯` buttons it sits between: all
          outline, all sized from `size="lg"`, all on the same 44px width
          floor. */}
      <PopoverTrigger render={<TriggerButton shape={variant} />}>
        <TriggerFace variant={variant} />
      </PopoverTrigger>
      <PopoverContent
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
        <button
          type="button"
          // shrink-0: the footer never gives up height to the list; when the
          // popup is at its viewport cap it is the list that scrolls.
          className="flex w-full shrink-0 items-center gap-2 border-t px-3 py-2 text-left text-sm text-muted-foreground hover:text-foreground"
          onClick={() => {
            setOpen(false)
            openMacrosDialog()
          }}
        >
          <SquarePen className="size-3.5 shrink-0" />
          Edit macros…
        </button>
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
                  ranges={subsequenceMatchRanges(macro.name, search)}
                />
              </span>
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
    </Command>
  )
}

type TriggerVariant = "labeled" | "icon" | "pill"

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
  if (shape === "icon") {
    return (
      <Button
        variant="outline"
        size="lg"
        aria-label="Run a macro"
        {...props}
        className={cn("min-w-11 shrink-0", className)}
      />
    )
  }
  return (
    <Button
      variant="outline"
      aria-label="Run a macro"
      {...props}
      className={className}
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
