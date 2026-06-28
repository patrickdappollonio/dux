import { useState } from "react"
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
import { macrosForTarget } from "@/lib/macros"
import { openMacrosDialog, runMacro, useDux } from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"

// A small quick-picker for sending a text macro to the focused target. Mirrors
// the TUI's Ctrl-\ macro bar: a filterable list of macros restricted to the
// focused target's surface, run by Enter/click. The verbose `Sent macro
// "<name>".` confirmation rides the existing status lane (toast) — no bespoke
// toast here.
//
// LAYOUT SAFETY: the trigger button is rendered by `TerminalPane` as an
// absolutely-positioned sibling of the xterm host (NOT inside the unpadded
// `containerRef` xterm opens into), so it never changes the terminal's box
// measurement — the same placement the fullscreen button uses. See the hostRef
// comment in `TerminalPane`.
export function MacroPopover({
  target,
  finalFocus,
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
}) {
  const { bootstrap } = useDux()
  const [open, setOpen] = useState(false)

  const allMacros = bootstrap?.macros ?? []
  const macros = macrosForTarget(allMacros, target)

  function handleRun(name: string) {
    // Phase 5: the macro's payload is written to the focused PTY socket (the
    // `target` this picker is filtered for), resolved by name in the store.
    runMacro(name)
    setOpen(false)
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      {/* Ellipsis on the label signals the button opens a menu of choices
          (unlike the fullscreen button, which acts immediately). */}
      <PopoverTrigger
        render={<Button variant="secondary" aria-label="Run a macro" />}
      >
        <SquareSlash />
        Macros…
      </PopoverTrigger>
      <PopoverContent align="end" className="w-72 p-0" finalFocus={finalFocus}>
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
