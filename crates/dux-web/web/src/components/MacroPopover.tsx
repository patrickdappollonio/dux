import { useRef, useState } from "react"
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
import { getComposeInsertSink } from "@/lib/composeInsert"
import { useIsMobile } from "@/hooks/use-mobile"
import { macrosForTarget } from "@/lib/macros"
import { openMacrosDialog, runMacro, useDux } from "@/lib/store"
import { getTerminalFocusElement } from "@/lib/terminalFocus"
import type { SelectedTarget } from "@/lib/store"

// A quick-picker for sending a text macro to the focused target: a filterable
// list restricted to that target's surface, run by Enter or click. There is
// deliberately no confirmation and no status; `runMacro` writes the payload
// straight to the focused PTY socket, or splices it into the compose draft
// while that is the typing surface, and the feedback is the text appearing.
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
  // the user to review and press Enter to submit — with the default
  // trigger-return, Enter would just re-press this trigger and re-open the menu.
  // This intentionally overrides the usual "return focus to the trigger" popover
  // convention because the trigger floats over a live terminal the user drives.
  finalFocus?: () => HTMLElement | null
  // "labeled" is the desktop trigger; "icon" is the phone header's icon button,
  // which passes no finalFocus so a dismissal returns focus to the trigger
  // rather than popping the soft keyboard (a pick landing in the compose draft
  // is the exception, in `resolveFinalFocus`); "pill" is the theater pill's
  // round ghost button, since a square outline button inside that one rounded
  // control would read as a second surface.
  variant?: "labeled" | "icon" | "pill"
}) {
  const { bootstrap } = useDux()
  const [open, setOpen] = useState(false)
  const isMobile = useIsMobile()
  // Set when the LAST pick landed in the mobile compose draft rather than the
  // PTY, consumed by the close-focus resolver below. A ref, not state: it is
  // read during Base UI's close-focus pass, never rendered.
  const pickedIntoComposeRef = useRef(false)

  const allMacros = bootstrap?.macros ?? []
  const macros = macrosForTarget(allMacros, target)

  function handleRun(name: string) {
    // The payload goes to the focused PTY socket, or into the compose draft
    // while that bar is the typing surface. The returned destination says which
    // happened, which steers the close focus below.
    pickedIntoComposeRef.current = runMacro(name) === "compose"
    setOpen(false)
  }

  // Where focus lands when the popover closes: into the compose draft for a
  // pick that landed there, because Base UI owns focus during a close and would
  // otherwise yank the keyboard back to the trigger. Otherwise the caller's
  // finalFocus, else the mounted terminal pane's typing surface for the desktop
  // labelled trigger, else Base UI's return-to-trigger.
  //
  // The desktop trigger sits in the header outside `TerminalPane` and has no
  // ref to hand in, so the pane registers its surface on the module-scope
  // `terminalFocus` hand-off. That fallback is not applied to the `icon`
  // variant, where focusing a terminal textarea pops the soft keyboard.
  function resolveFinalFocus(): HTMLElement | boolean | null {
    if (pickedIntoComposeRef.current) {
      pickedIntoComposeRef.current = false
      const composeTarget = getComposeInsertSink()?.target() ?? null
      if (composeTarget) return composeTarget
    }
    if (finalFocus) return finalFocus()
    if (variant === "labeled") return getTerminalFocusElement() ?? true
    return true
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
      <PopoverTrigger
        render={
          variant === "pill" ? (
            <Button
              variant="ghost"
              size="icon"
              className="size-10 shrink-0 rounded-full"
              aria-label="Run a macro"
            />
          ) : variant === "icon" ? (
            <Button
              variant="outline"
              size="lg"
              className="min-w-11 shrink-0"
              aria-label="Run a macro"
            />
          ) : (
            <Button variant="outline" aria-label="Run a macro" />
          )
        }
      >
        <SquareSlash />
        {variant === "labeled" ? <>Macros…</> : null}
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
            No macros found — start by creating one!
          </div>
        ) : macros.length === 0 ? (
          <div className="px-3 py-6 text-center text-sm text-muted-foreground">
            No macros for this target kind — add one via Edit macros below.
          </div>
        ) : (
          // min-h-0 lets the Command column shrink when the popup hits its
          // viewport cap, so the list below scrolls instead of pushing the
          // Edit-macros footer off screen.
          <Command className="min-h-0">
            {/* No autofocus in the sheet: menus never pop the soft keyboard
                on open, and this now presents as one of them; tapping the
                field summons the keyboard. Desktop keeps type-immediately
                (the filters-are-type-immediately convention). */}
            <CommandInput placeholder="Search macros…" autoFocus={!isMobile} />
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
                    onSelect={() => handleRun(macro.name)}
                  >
                    {macro.name}
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
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
