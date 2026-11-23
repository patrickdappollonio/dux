import type { ReactNode } from "react"

import { cn } from "@/lib/utils"

// The rounded muted square that holds an icon (or a step number) beside a
// title. It is decoration: the words next to it say everything, so it is
// always hidden from a screen reader.
//
// `use` says whether the tile sits inside something that can be pressed.
// "action" gets the hairline border and the slightly softer glyph of a button
// face; "display" is a flat tile that only reads.
const SIZES = {
  // A numbered step in running text.
  sm: "size-6 rounded-md text-xs font-medium [&_svg]:size-3.5",
  // A launcher row, or a masthead logo, on a computer.
  md: "size-9 rounded-lg [&_svg]:size-[18px]",
  // The same row on a phone, where everything sits larger.
  lg: "size-10 rounded-lg [&_svg]:size-5",
} as const

export type IconTileSize = keyof typeof SIZES

export function IconTile({
  size,
  use,
  children,
}: {
  size: IconTileSize
  use: "action" | "display"
  children: ReactNode
}) {
  return (
    <span
      aria-hidden="true"
      data-use={use}
      className={cn(
        "flex shrink-0 items-center justify-center bg-muted",
        SIZES[size],
        use === "action"
          ? "border border-border/60 text-foreground/90"
          : "text-foreground",
      )}
    >
      {children}
    </span>
  )
}
