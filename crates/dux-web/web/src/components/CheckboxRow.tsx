import type { ReactNode } from "react"

import { Checkbox } from "@/components/ui/checkbox"
import { CHECK_LABEL_TEXT, FIELD_HELP_TEXT } from "@/components/ui/form-text"
import { cn } from "@/lib/utils"

interface CheckboxRowProps {
  id: string
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  disabled?: boolean
  children: ReactNode
  /** A line under the words, read with the box. */
  help?: ReactNode
}

// A dialog's option row: the box and a label that is the whole rest of the row.
// At phone width and under a coarse pointer the row is the 40px touch floor,
// the label stretches across its height, and the box's invisible tap margin
// grows to the same 40px (13px per side, counted from inside its 1px border),
// so a tap anywhere on the row lands on the control. The label's text sits in
// a span of its own so a sentence with names in it wraps as one run rather
// than as flex items, and a disabled box dims its label with it.
export function CheckboxRow({
  id,
  checked,
  onCheckedChange,
  disabled,
  children,
  help,
}: CheckboxRowProps) {
  const helpId = help != null ? `${id}-help` : undefined
  const row = (
    <div className="flex items-center gap-2 max-md:min-h-10 pointer-coarse:min-h-10">
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
        aria-describedby={helpId}
        className="max-md:after:-inset-y-[13px] pointer-coarse:after:-inset-y-[13px]"
      />
      <label
        htmlFor={id}
        className={cn(
          "flex min-w-0 flex-1 items-center self-stretch peer-data-disabled:cursor-not-allowed peer-data-disabled:opacity-50",
          CHECK_LABEL_TEXT,
        )}
      >
        <span className="min-w-0">{children}</span>
      </label>
    </div>
  )
  if (helpId === undefined) return row
  // The help starts under the words: the box's 16px plus the row's 8px gap.
  return (
    <div className="flex flex-col gap-1">
      {row}
      <div id={helpId} className={cn("pl-6", FIELD_HELP_TEXT)}>
        {help}
      </div>
    </div>
  )
}
