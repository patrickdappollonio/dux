import { Switch as SwitchPrimitive } from "@base-ui/react/switch"

import { cn } from "@/lib/utils"

// A settings toggle is visually a switch, not a checkbox, and base-ui ships
// one, so per the "shadcn/base-ui first" tenet we wrap it rather than reusing
// Checkbox. The visible switch is the same on every device; at phone width
// and under a coarse pointer an invisible 10px margin per side widens its tap
// area to 38px tall (measured from inside its 1px border), under the floor on
// that axis: in Preferences the rows above and below it are text, and the
// nearest other control is 44px or more away.
function Switch({ className, ...props }: SwitchPrimitive.Root.Props) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent shadow-xs transition-colors outline-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 data-[checked]:bg-primary data-[unchecked]:bg-input max-md:after:absolute max-md:after:-inset-2.5 pointer-coarse:after:absolute pointer-coarse:after:-inset-2.5",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="pointer-events-none block size-4 rounded-full bg-background shadow-lg ring-0 transition-transform data-[checked]:translate-x-4 data-[unchecked]:translate-x-0.5"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
