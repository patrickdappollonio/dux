import * as React from "react"
import { Search } from "lucide-react"

type SearchFieldInputProps = Omit<
  React.ComponentProps<"input">,
  "type" | "className" | "style" | "value" | "onChange" | "placeholder" | "aria-label" | "aria-labelledby" | "ref"
>

export interface SearchFieldProps extends SearchFieldInputProps {
  value: string
  onChange: (value: string) => void
  /** The field's accessible name. */
  label: string
  /** What the empty field says; the label when left out. */
  placeholder?: string
  /** What Escape does with typed text. `passes` leaves it alone, so the key
   * reaches whatever owns it (a dialog that closes on it); `clears` empties
   * the field and lets the key go on. */
  escape?: "passes" | "clears"
  inputRef?: React.Ref<HTMLInputElement>
}

/**
 * The one search or filter field: a magnifier, the text, a rounded outline.
 *
 * It types immediately, with no engage step. The input is plain text and never
 * a search-type input: that clears itself on Escape in a way a dialog around it
 * cannot see, and Chromium draws a clear button of its own in it. It says what
 * a search-type input would (a search role, the Search key) by attributes.
 */
export function SearchField({
  value,
  onChange,
  label,
  placeholder,
  escape = "passes",
  inputRef,
  onKeyDown,
  ...inputProps
}: SearchFieldProps) {
  return (
    // A label, so a press anywhere in the box lands in the field. The height
    // is a minimum the input fills: 32px, and 40px on a phone and under a
    // finger at any width.
    <label className="flex min-h-8 w-full min-w-0 items-center gap-2 rounded-md border border-input bg-input/30 px-3 transition-colors has-[input:focus-visible]:border-ring has-[input:focus-visible]:ring-3 has-[input:focus-visible]:ring-ring/50 max-md:min-h-10 pointer-coarse:min-h-10">
      <Search aria-hidden className="size-4 shrink-0 text-muted-foreground" />
      <input
        // Announced as a search field and asks a phone keyboard for its Search
        // key, as a search-type input does. A caller's own role (the language
        // picker's combobox) replaces the first.
        role="searchbox"
        enterKeyHint="search"
        {...inputProps}
        ref={inputRef}
        type="text"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          onKeyDown?.(event)
          if (escape === "clears" && event.key === "Escape" && !event.defaultPrevented && value !== "") {
            onChange("")
          }
        }}
        placeholder={placeholder ?? label}
        aria-label={label}
        // text-base on phones so iOS Safari does not zoom the page when the
        // field takes focus (the Input primitive's convention).
        className="min-w-0 flex-1 self-stretch bg-transparent text-base outline-none placeholder:text-muted-foreground md:text-sm"
      />
    </label>
  )
}
