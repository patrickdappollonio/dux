import { SquareCode } from "lucide-react"

import { EDITOR_ICON_PATHS } from "@/lib/editorIcons"
import { cn } from "@/lib/utils"

// Monochrome brand glyph for an editor menu entry, keyed by the dux-core editor
// config key (e.g. "vscode"). Renders the vendored simple-icons path with
// `fill="currentColor"` so it inherits the menu's text color. Falls back to a
// neutral lucide glyph for any key without a bundled icon, so the menu never
// shows a blank — new editors degrade gracefully until an icon is added.
export function EditorIcon({
  editorKey,
  className,
}: {
  editorKey: string
  className?: string
}) {
  const path = EDITOR_ICON_PATHS[editorKey]
  if (!path) return <SquareCode className={className} />
  return (
    <svg
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={cn("size-4", className)}
    >
      <path d={path} />
    </svg>
  )
}
