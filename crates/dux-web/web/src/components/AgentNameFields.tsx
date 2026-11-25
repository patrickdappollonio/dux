// The agent-name controls every dialog that takes an agent's name shares: the
// name input with its pet-name spinner, and the "Use randomized pet name"
// checkbox. Each dialog hands in its own value and actions, so none can drift
// on how a name is typed or sanitized.

import { type ChangeEvent, type CompositionEvent, type KeyboardEvent, useState } from "react"

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
  // The text of an IME composition in progress, shown as typed: rewriting it
  // (a space turned into a dash) would break the composition.
  const [composing, setComposing] = useState<string | null>(null)

  function report(input: HTMLInputElement): void {
    const raw = input.value
    const caret = input.selectionStart ?? raw.length
    onChange(raw)
    const sanitized = sanitizeAgentName(raw)
    if (sanitized === raw) return
    const next = Math.max(0, caret - (raw.length - sanitized.length))
    // Writing the sanitized value puts the caret at the end. React writes it
    // before this event's dispatch returns, so a microtask puts the caret back
    // before the next key can arrive; a later callback (an animation frame)
    // lands after a fast typist's next key and moves the caret back past it.
    queueMicrotask(() => input.setSelectionRange(next, next))
  }

  function handleChange(event: ChangeEvent<HTMLInputElement>): void {
    if (composing !== null || (event.nativeEvent as InputEvent).isComposing === true) {
      setComposing(event.target.value)
      return
    }
    report(event.target)
  }

  function handleCompositionStart(event: CompositionEvent<HTMLInputElement>): void {
    setComposing(event.currentTarget.value)
  }

  function handleCompositionEnd(event: CompositionEvent<HTMLInputElement>): void {
    setComposing(null)
    report(event.currentTarget)
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
        value={composing ?? value}
        onChange={handleChange}
        onCompositionStart={handleCompositionStart}
        onCompositionEnd={handleCompositionEnd}
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
