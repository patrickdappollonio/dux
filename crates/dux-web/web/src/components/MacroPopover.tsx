import { useState } from "react"
import { SquareSlash } from "lucide-react"

import {
  Command,
  CommandEmpty,
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
import { paletteShortcutLabel } from "@/lib/platform"

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
export function MacroPopover({ target }: { target: SelectedTarget }) {
  const { viewModel } = useDux()
  const [open, setOpen] = useState(false)

  const allMacros = viewModel?.macros ?? []
  const macros = macrosForTarget(allMacros, target)
  const targetId =
    target.kind === "terminal" ? target.terminalId : target.sessionId

  function handleRun(name: string) {
    runMacro(targetId, name)
    setOpen(false)
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            variant="secondary"
            size="icon"
            title="Run a macro"
            aria-label="Run a macro"
          />
        }
      >
        <SquareSlash />
      </PopoverTrigger>
      <PopoverContent align="end" className="w-72 p-0">
        {allMacros.length === 0 ? (
          <div className="p-3 text-sm text-muted-foreground">
            No macros yet — open Edit macros from the command palette (
            {paletteShortcutLabel()}).
          </div>
        ) : macros.length === 0 ? (
          <div className="p-3 text-sm text-muted-foreground">
            No macros for this target kind.
          </div>
        ) : (
          <Command>
            <CommandInput placeholder="Search macros…" autoFocus />
            <CommandList>
              <CommandEmpty>No matching macros.</CommandEmpty>
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
            </CommandList>
          </Command>
        )}
        <button
          type="button"
          className="w-full border-t px-3 py-2 text-left text-xs text-muted-foreground hover:text-foreground"
          onClick={() => {
            setOpen(false)
            openMacrosDialog()
          }}
        >
          Edit macros…
        </button>
      </PopoverContent>
    </Popover>
  )
}
