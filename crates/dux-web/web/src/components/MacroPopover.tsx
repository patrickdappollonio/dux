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
import { macrosForTarget } from "@/lib/macros"
import { openMacrosDialog, runMacro, useDux } from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"

// A small quick-picker for sending a text macro to the focused target. Mirrors
// the TUI's Ctrl-\ macro bar: a filterable list of macros restricted to the
// focused target's surface, run by Enter/click. There is deliberately NO
// confirmation of any kind here: `runMacro` writes the payload straight to the
// focused PTY socket — or, while the mobile compose bar is the typing surface,
// splices it into the compose DRAFT — and emits no status, so nothing reaches
// the toast lane. (The TUI's `Sent macro "<name>".` status line has no web
// counterpart, because there is no server round trip to carry one; the
// feedback is the macro text appearing at the prompt or in the draft.)
//
// LAYOUT SAFETY: the trigger button is rendered by `TerminalPane` as an
// absolutely-positioned sibling of the xterm host (NOT inside the unpadded
// `containerRef` xterm opens into), so it never changes the terminal's box
// measurement. See the hostRef comment in `TerminalPane`.
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
  // "labeled" is the desktop floating trigger ("Macros…" over the pane);
  // "icon" is the mobile terminal-screen header's icon button, matching the
  // header's outline action-cluster idiom (see the trigger below). On phones
  // the picker submits through
  // the compose bar / a tap, so the icon variant passes no finalFocus and
  // keeps the default return-focus-to-trigger behavior on a dismissal rather
  // than popping the soft keyboard by focusing a terminal textarea. A PICK
  // that landed in the compose draft is the one exception, resolved in
  // `resolveFinalFocus` below: focus follows the macro into the draft.
  variant?: "labeled" | "icon"
}) {
  const { bootstrap } = useDux()
  const [open, setOpen] = useState(false)
  // Set when the LAST pick landed in the mobile compose draft rather than the
  // PTY, consumed by the close-focus resolver below. A ref, not state: it is
  // read during Base UI's close-focus pass, never rendered.
  const pickedIntoComposeRef = useRef(false)

  const allMacros = bootstrap?.macros ?? []
  const macros = macrosForTarget(allMacros, target)

  function handleRun(name: string) {
    // Phase 5: the macro's payload is written to the focused PTY socket (the
    // `target` this picker is filtered for), resolved by name in the store —
    // unless the mobile compose bar is the typing surface, in which case the
    // store splices the text into the compose draft instead (the returned
    // destination says which happened, steering the close focus below).
    pickedIntoComposeRef.current = runMacro(name) === "compose"
    setOpen(false)
  }

  // Where focus lands when the popover closes. A pick that landed in the
  // compose draft moves focus INTO that draft (the pane's insert already asked
  // for it, but Base UI owns focus during a popover close and would otherwise
  // hand it back to the trigger, yanking the keyboard away from the text the
  // user is about to edit). Every other close keeps the existing behavior: the
  // caller's finalFocus when given (the desktop trigger points it at xterm's
  // textarea), else Base UI's default return-to-trigger (`true`).
  function resolveFinalFocus(): HTMLElement | boolean | null {
    if (pickedIntoComposeRef.current) {
      pickedIntoComposeRef.current = false
      const composeTarget = getComposeInsertSink()?.target() ?? null
      if (composeTarget) return composeTarget
    }
    return finalFocus ? finalFocus() : true
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      {/* Ellipsis on the label signals the button opens a menu of choices
          (rather than acting immediately). The icon variant drops the label:
          it sits in the mobile terminal header among other icon-only controls,
          and on a phone icon-only is the default because space is scarce. Do
          not give it a label back.

          The shape lives HERE rather than being overridden at the call site,
          so the trigger cannot drift from the `±N` and `⋯` buttons it sits
          between. All three are `outline` (the desktop AppMenu cog and the
          Show-Changes button beside it already establish outline as the
          one-family treatment for an action cluster), all three take their
          height from `size="lg"`, and all three carry the same 44px width
          floor. See MobileShell.tsx's header for the full justification. */}
      <PopoverTrigger
        render={
          variant === "icon" ? (
            <Button
              variant="outline"
              size="lg"
              className="min-w-11 shrink-0"
              aria-label="Run a macro"
            />
          ) : (
            <Button variant="secondary" aria-label="Run a macro" />
          )
        }
      >
        <SquareSlash />
        {variant === "icon" ? null : <>Macros…</>}
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-72 p-0"
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
          <Command>
            <CommandInput placeholder="Search macros…" autoFocus />
            {/* CommandGroup's padding puts breathing room between the search
                field and the first result, matching the gap above the Edit
                macros footer below. */}
            <CommandList>
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
          className="flex w-full items-center gap-2 border-t px-3 py-2 text-left text-sm text-muted-foreground hover:text-foreground"
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
