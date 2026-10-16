import type { ReactNode } from "react"

import { Checkbox } from "@/components/ui/checkbox"

interface CheckboxRowProps {
  id: string
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  children: ReactNode
}

// A dialog's option row: the box and a label that is the whole rest of the row.
// On a coarse pointer the row is the 40px touch floor and the label stretches
// across all of it, so a tap anywhere on the row lands on the control; the
// label alone used to be 20px tall inside a 40px row, which left the tap to
// whatever was between them.
export function CheckboxRow({
  id,
  checked,
  onCheckedChange,
  children,
}: CheckboxRowProps) {
  return (
    <div className="flex items-center gap-2 max-md:min-h-10">
      <Checkbox id={id} checked={checked} onCheckedChange={onCheckedChange} />
      <label htmlFor={id} className="flex flex-1 items-center self-stretch text-sm">
        {children}
      </label>
    </div>
  )
}
