import { FolderGit2 } from "lucide-react"
import type { CSSProperties } from "react"

import { Spinner } from "@/components/Spinner"
import { ModalGroupHeading } from "@/components/modal"
import { useScrollEdges } from "@/hooks/use-scroll-edges"
import { INSET_FOCUS_RING } from "@/lib/focusRing"
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
 *
 * Until the server answers (`list` null) it holds the room the rows will take,
 * so the dialog does not change height under the pointer when they arrive:
 * one row per project the app already knows of, up to the cap, or the whole
 * cap while it knows of none yet.
 */
// The rows' cap, and the room they are given while they load. A row is 2.25rem
// with a fine pointer and 2.75rem (its 44px floor) on a phone or under a finger.
const ROWS_MAX_HEIGHT = "max-h-[8.75rem] max-md:max-h-[9.625rem] pointer-coarse:max-h-[9.625rem]"
const ROWS_HEIGHT = "h-[8.75rem] max-md:h-[9.625rem] pointer-coarse:h-[9.625rem]"
const ROWS_HEIGHT_FOR_COUNT =
  "h-[min(calc(var(--rows)*2.25rem),8.75rem)] max-md:h-[min(calc(var(--rows)*2.75rem),9.625rem)] pointer-coarse:h-[min(calc(var(--rows)*2.75rem),9.625rem)]"

export function ProjectShortcutList({
  list,
  expectedRows = null,
  pickedProjectId,
  onPick,
}: {
  list: ProjectShortcutListView | null
  /** How many rows to hold room for while `list` is null; null when unknown. */
  expectedRows?: number | null
  /** The project whose folder the picker currently targets, so its row paints
   * as chosen; null when the target is not a project pick. */
  pickedProjectId: string | null
  onPick: (row: ProjectShortcutView) => void
}) {
  const { ref, moreAbove, moreBelow } = useScrollEdges<HTMLDivElement>()
  if (list === null) {
    return (
      <div data-project-shortcuts-pending="" className="flex shrink-0 flex-col gap-1.5">
        <div className="flex flex-col overflow-hidden rounded-md border">
          <ModalGroupHeading inset>Your projects</ModalGroupHeading>
          <div
            className={`flex items-center justify-center ${expectedRows === null ? ROWS_HEIGHT : ROWS_HEIGHT_FOR_COUNT}`}
            style={expectedRows === null ? undefined : ({ "--rows": expectedRows } as CSSProperties)}
          >
            <Spinner />
          </div>
        </div>
        {/* The caption's line, kept until the server's words arrive. */}
        <p aria-hidden="true" className="text-xs">
          &nbsp;
        </p>
      </div>
    )
  }
  return (
    <div className="flex shrink-0 flex-col gap-1.5">
      <div className="flex flex-col overflow-hidden rounded-md border">
        <ModalGroupHeading inset>Your projects</ModalGroupHeading>
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
          className={`dux-list-fade flex flex-col overflow-y-auto ${ROWS_MAX_HEIGHT}`}
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
              className={`flex shrink-0 items-center gap-2 px-3 py-2 text-left text-sm enabled:hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 max-md:min-h-11 pointer-coarse:min-h-11 ${INSET_FOCUS_RING} ${
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
