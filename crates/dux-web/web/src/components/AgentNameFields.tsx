// The agent-name controls both name-taking dialogs share: the name input with
// its pet-name spinner, and the "Use randomized pet name" checkbox. The state
// behind them lives in the store, so each dialog hands in its own value and
// actions and the two cannot drift on how a name is typed or sanitized.

import type { ChangeEvent, KeyboardEvent } from "react"

import { GlyphSpinner } from "@/components/GlyphSpinner"
import { Checkbox } from "@/components/ui/checkbox"
import { Input } from "@/components/ui/input"
import { sanitizeAgentName } from "@/lib/agentName"

interface AgentNameInputProps {
  value: string
  /** Receives the raw text; the store sanitizes it. */
  onChange: (raw: string) => void
  onSubmit: () => void
  placeholder: string
  invalid: boolean
  /** A generated name is on its way: the field waits and shows the spinner. */
  generating: boolean
  autoFocus: boolean
  ariaLabel?: string
}

export function AgentNameInput({
  value,
  onChange,
  onSubmit,
  placeholder,
  invalid,
  generating,
  autoFocus,
  ariaLabel,
}: AgentNameInputProps) {
  function handleChange(event: ChangeEvent<HTMLInputElement>): void {
    const input = event.target
    const raw = input.value
    const caret = input.selectionStart ?? raw.length
    onChange(raw)
    const sanitized = sanitizeAgentName(raw)
    if (sanitized === raw) return
    const next = Math.max(0, caret - (raw.length - sanitized.length))
    // Controlled sanitization moves the caret to the end; restore its adjusted
    // position after removed characters shorten a mid-string edit.
    requestAnimationFrame(() => input.setSelectionRange(next, next))
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.key !== "Enter") return
    event.preventDefault()
    onSubmit()
  }

  return (
    <div className="relative">
      <Input
        value={value}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        aria-label={ariaLabel}
        aria-invalid={invalid}
        disabled={generating}
        autoFocus={autoFocus}
      />
      {generating && (
        <GlyphSpinner className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
      )}
    </div>
  )
}

interface RandomizeNameCheckboxProps {
  checked: boolean
  onToggle: () => void
}

export function RandomizeNameCheckbox({
  checked,
  onToggle,
}: RandomizeNameCheckboxProps) {
  return (
    <div className="flex items-center gap-2">
      <Checkbox
        id="randomize-agent-name"
        checked={checked}
        onCheckedChange={onToggle}
      />
      <label htmlFor="randomize-agent-name" className="text-sm">
        Use randomized pet name
      </label>
    </div>
  )
}
