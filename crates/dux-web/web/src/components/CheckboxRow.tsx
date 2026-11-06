import type { ReactNode } from "react"

import { Checkbox } from "@/components/ui/checkbox"

interface CheckboxRowProps {
  id: string
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  disabled?: boolean
  children: ReactNode
}

// A dialog's option row: the box and a label that is the whole rest of the row.
// On a coarse pointer the row is the 40px touch floor and the label stretches
// across all of it, so a tap anywhere on the row lands on the control; the
// label alone used to be 20px tall inside a 40px row, which left the tap to
// whatever was between them. The label's text sits in a span of its own so a
// sentence with names in it wraps as one run rather than as flex items, and a
// disabled box dims its label with it.
export function CheckboxRow({
  id,
  checked,
  onCheckedChange,
  disabled,
  children,
}: CheckboxRowProps) {
  return (
    <div className="flex items-center gap-2 max-md:min-h-10">
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
      />
      <label
        htmlFor={id}
        className="flex min-w-0 flex-1 items-center self-stretch text-sm peer-data-disabled:cursor-not-allowed peer-data-disabled:opacity-50"
      >
        <span className="min-w-0">{children}</span>
      </label>
    </div>
  )
}
