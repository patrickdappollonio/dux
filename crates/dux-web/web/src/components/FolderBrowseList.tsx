import { CornerLeftUp, Folder, FolderGit2, FolderOpen } from "lucide-react"

import { BrailleSpinner } from "@/components/BrailleSpinner"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import { baseName } from "@/lib/paths"
import type { DirEntryView } from "@/lib/types"

/** The folder name as a monospace pill: the row's actual target, told apart
 * from the surrounding prose. */
export function FolderPill({ name }: { name: string }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1 rounded-full border border-border bg-muted px-2 py-0.5 font-mono text-xs">
      <Folder className="size-3 shrink-0 text-muted-foreground" />
      <span className="truncate">{name}</span>
    </span>
  )
}

/**
 * The server-side directory browser's LIST: a pinned "commit this folder" row,
 * a divider, then the navigable entries.
 *
 * Extracted because two dialogs pick a folder this way and the picking itself
 * is identical: only what happens to the chosen folder differs (adding a
 * project runs an inspection ladder; a standalone agent runs in whatever is
 * there). Everything decision-shaped stays with the caller; this owns the
 * rows, the touch targets, the git badge and the parent affordance, so the two
 * pickers cannot drift into looking like different features.
 *
 * `commitLabel` is the pinned row's verb, because it is the one word that says
 * what the picker is for.
 */
export function FolderBrowseList({
  path,
  entries,
  loading,
  commitLabel,
  committed,
  onCommit,
  onOpen,
}: {
  path: string
  entries: DirEntryView[]
  loading: boolean
  commitLabel: string
  /** True while the pinned row's folder is the caller's current target, so the
   * row paints as chosen. */
  committed: boolean
  onCommit: (path: string) => void
  onOpen: (entry: DirEntryView) => void
}) {
  return (
    <ScrollArea className="h-[50vh] rounded-md border md:h-80">
      {loading ? (
        <div className="flex h-[50vh] items-center justify-center md:h-80">
          <BrailleSpinner className="text-lg text-muted-foreground" />
        </div>
      ) : (
        <div className="flex flex-col">
          {/* Pinned, client-synthesized row: the ONLY way the current directory
              becomes the target. The footer stays strictly selection-driven, so
              the primary button never acts on wherever the user happens to be
              standing. The pinned band (a faint elevated tint plus a neutral
              left accent rule) and the monospace target pill read this as a
              commit action, distinct from the ordinary folder rows below. */}
          <button
            type="button"
            onClick={() => onCommit(path)}
            className={`flex min-h-11 items-center gap-2 border-l-2 border-primary/60 px-3 py-2 text-left text-sm hover:bg-accent md:min-h-0 ${
              committed ? "bg-accent" : "bg-muted/40"
            }`}
          >
            <FolderOpen className="size-4 shrink-0 text-muted-foreground" />
            <span className="shrink-0">{commitLabel}</span>
            <FolderPill name={baseName(path)} />
          </button>
          {/* Non-interactive divider separating the commit action from the
              navigation list below. */}
          <div className="px-3 py-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            Browse
          </div>
          {entries.map((entry) => {
            // The synthetic parent ("../") row reads as an "up" action, not a
            // folder: a distinct glyph and the parent's basename. It shows no
            // path, because the header field above is the authoritative path
            // for where you are and repeating it here only widened the row and
            // duplicated that path. No git badge.
            if (entry.is_parent) {
              return (
                <button
                  key={entry.path}
                  type="button"
                  onClick={() => onOpen(entry)}
                  // min-h-11 on phones gives each row a ≥44px touch target;
                  // desktop keeps the compact py-2 density via md:.
                  className="flex min-h-11 items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent md:min-h-0"
                >
                  <CornerLeftUp className="size-4 shrink-0 text-muted-foreground" />
                  <span className="shrink-0">Up to</span>
                  <FolderPill name={baseName(entry.path)} />
                </button>
              )
            }
            // No row-level selected state: every folder row navigates, and the
            // target lives solely on the pinned row above. Every folder opens
            // on click, git repository or not, matching the terminal UI's
            // navigate-anywhere model: a repository is not a dead end.
            const Icon = entry.is_git_repo ? FolderGit2 : Folder
            return (
              <button
                key={entry.path}
                type="button"
                onClick={() => onOpen(entry)}
                className="flex min-h-11 items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent md:min-h-0"
              >
                <Icon className="size-4 shrink-0 text-muted-foreground" />
                <span className="flex-1 truncate">{entry.label}</span>
                {entry.is_git_repo ? (
                  <Badge variant="secondary" className="shrink-0">
                    git
                  </Badge>
                ) : null}
              </button>
            )
          })}
        </div>
      )}
    </ScrollArea>
  )
}
