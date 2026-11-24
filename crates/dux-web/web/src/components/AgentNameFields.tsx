// The agent-name controls both name-taking dialogs share: the name input with
// its pet-name spinner, and the "Use randomized pet name" checkbox. The state
// behind them lives in the store, so each dialog hands in its own value and
// actions and the two cannot drift on how a name is typed or sanitized.

import type { ChangeEvent, KeyboardEvent } from "react"

import { Spinner } from "@/components/Spinner"
import { CheckboxRow } from "@/components/CheckboxRow"
import type { TextBoxWiring } from "@/components/ui/form-group"
import { Input } from "@/components/ui/input"
import { sanitizeAgentName } from "@/lib/agentName"

interface AgentNameInputProps {
  value: string
  /** Receives the raw text; the store sanitizes it. */
  onChange: (raw: string) => void
  onSubmit: () => void
  /** What the input shows while empty; left out where a label says what it takes. */
  placeholder?: string
  /** A generated name is on its way: the field waits and shows the spinner. */
  generating: boolean
  autoFocus: boolean
  /** The `FormGroup` wiring that names the input and reads its help and error with it. */
  field: TextBoxWiring
}

export function AgentNameInput({
  value,
  onChange,
  onSubmit,
  placeholder,
  generating,
  autoFocus,
  field,
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
        {...field}
        value={value}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        disabled={generating}
        autoFocus={autoFocus}
      />
      {generating && (
        <span className="absolute right-3 top-1/2 flex -translate-y-1/2">
          <Spinner kind="glyph" size="text" tone="muted" />
        </span>
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
    <CheckboxRow
      id="randomize-agent-name"
      checked={checked}
      onCheckedChange={onToggle}
    >
      Use randomized pet name
    </CheckboxRow>
  )
}
