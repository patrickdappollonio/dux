import {
  AlertIcon,
  DiffAddedIcon,
  DiffModifiedIcon,
  DiffRemovedIcon,
  DiffRenamedIcon,
  FileIcon,
} from "@primer/octicons-react"

import { fileStatusMeta, type FileStatusKind } from "@/lib/changedFiles"
import { cn } from "@/lib/utils"
import { SimpleTooltip } from "@/components/SimpleTooltip"

// One marker for a file's git status, shared by the changes pane and the
// editor's tree/search so the marker reads identically everywhere. We use
// GitHub's Octicons diff glyphs — the same icons GitHub renders in PR file lists
// — colored GitHub-style, so the status is recognizable at a glance instead of
// requiring an invented pictograph→meaning mapping. The pure, unit-tested
// `fileStatusMeta` maps the raw status to a kind + label; the two Records below
// (keyed by FileStatusKind) map the kind to its octicon and color, so adding a
// kind is a compile error until both are supplied.
const ICONS: Record<FileStatusKind, typeof DiffAddedIcon> = {
  modified: DiffModifiedIcon,
  added: DiffAddedIcon,
  deleted: DiffRemovedIcon,
  renamed: DiffRenamedIcon,
  copied: DiffRenamedIcon,
  conflict: AlertIcon,
  "type-changed": DiffModifiedIcon,
  untracked: DiffAddedIcon,
  other: FileIcon,
}

// Colors mirror GitHub's diff palette and the +/- line-count colors used in the
// same row: added/untracked green, deleted red, modified/type-change amber,
// renamed/copied blue, conflict orange. `other` stays neutral.
const COLORS: Record<FileStatusKind, string> = {
  modified: "text-amber-500",
  added: "text-green-500",
  deleted: "text-red-500",
  renamed: "text-blue-500",
  copied: "text-blue-500",
  conflict: "text-orange-500",
  "type-changed": "text-amber-500",
  untracked: "text-green-500",
  other: "text-muted-foreground",
}

// `tooltip` defaults to true, which is what every ordinary site wants. It is
// turned OFF only where the marker cannot own its own hover: the changes pane
// stacks it under the selection checkbox and makes it pointer-transparent, so
// the tooltip moves out to the slot around both and this one would be dead
// weight (and a second popup on the same 20px box).
export function FileStatusIcon({
  status,
  tooltip = true,
}: {
  status: string
  tooltip?: boolean
}) {
  const { kind, label } = fileStatusMeta(status)
  const Icon = ICONS[kind]
  const glyph = (
    <span
      role="img"
      aria-label={label}
      className={cn("inline-flex shrink-0", COLORS[kind])}
    >
      <Icon size={14} />
    </span>
  )
  if (!tooltip) return glyph
  return <SimpleTooltip content={label}>{glyph}</SimpleTooltip>
}
