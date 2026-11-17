import { FolderGit2 } from "lucide-react"

import { useScrollEdges } from "@/hooks/use-scroll-edges"
import type {
  ProjectShortcutListView,
  ProjectShortcutView,
} from "@/lib/types"

/**
 * The New standalone agent picker's project shortcuts: your projects, above the
 * folder explorer, each one a way to send the explorer to that project's folder.
 *
 * Render-only on purpose. The rows, their order, which cannot be picked, the
 * words for why and the caption all arrive from the server
 * (`dux_core::standalone_shortcuts`), the same list the terminal UI draws, so
 * the two surfaces cannot disagree. Nothing here filters, sorts or judges a row.
 *
 * A row that cannot be picked is a disabled button: it takes no click and no
 * focus, and says why in the server's words. On a phone the reason sits on a
 * line of its own under the name, so it never squeezes the folder down to
 * nothing; from the `md` breakpoint up it trails the folder on the same line.
 *
 * The rows scroll under their heading once there are more than fit, and the
 * edge with rows hidden past it fades out (`.dux-list-fade`, the agents list's
 * own cue), so a list cut at a row boundary still says there is more.
 */
export function ProjectShortcutList({
  list,
  pickedProjectId,
  onPick,
}: {
  list: ProjectShortcutListView
  /** The project whose folder the picker currently targets, so its row paints
   * as chosen; null when the target is not a project pick. */
  pickedProjectId: string | null
  onPick: (row: ProjectShortcutView) => void
}) {
  const { ref, moreAbove, moreBelow } = useScrollEdges<HTMLDivElement>()
  return (
    <div className="flex shrink-0 flex-col gap-1.5">
      <div className="flex flex-col overflow-hidden rounded-md border">
        <div className="px-3 py-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Your projects
        </div>
        {/* Capped and scrolling by itself, so a long project list takes the
            explorer's room only up to a point. The cap is about three and a
            half coarse rows on a phone or any coarse pointer (44px each), and a
            little under four compact rows with a fine pointer, so the next
            row peeks; a taller two-line row can move the cut, and the fade is
            what says more rows follow wherever it lands. */}
        <div
          ref={ref}
          data-project-shortcut-rows
          data-more-above={moreAbove ? "" : undefined}
          data-more-below={moreBelow ? "" : undefined}
          className="dux-list-fade flex max-h-[8.75rem] flex-col overflow-y-auto max-md:max-h-[9.625rem] pointer-coarse:max-h-[9.625rem]"
        >
          {list.rows.map((row) => (
            <button
              key={row.project_id}
              type="button"
              data-project-shortcut={row.project_id}
              data-picked={pickedProjectId === row.project_id ? "" : undefined}
              disabled={row.reason !== null}
              onClick={() => onPick(row)}
              // The touch floor on a phone's width and on any coarse pointer; a
              // fine pointer at desktop width keeps the compact py-2 density.
              className={`flex shrink-0 items-center gap-2 px-3 py-2 text-left text-sm enabled:hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 max-md:min-h-11 pointer-coarse:min-h-11 ${
                pickedProjectId === row.project_id ? "bg-accent" : ""
              }`}
            >
              <FolderGit2 className="size-4 shrink-0 text-muted-foreground" />
              <span className="flex min-w-0 flex-1 flex-col md:flex-row md:items-center md:gap-2">
                <span className="flex min-w-0 items-center gap-2 md:flex-1">
                  <span className="max-w-[60%] shrink-0 truncate">
                    {row.name}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">
                    {row.folder_label}
                  </span>
                </span>
                {row.reason ? (
                  <span
                    data-shortcut-reason
                    className="text-xs text-muted-foreground md:shrink-0"
                  >
                    {row.reason}
                  </span>
                ) : null}
              </span>
            </button>
          ))}
        </div>
      </div>
      <p className="text-xs text-muted-foreground">{list.caption}</p>
    </div>
  )
}
