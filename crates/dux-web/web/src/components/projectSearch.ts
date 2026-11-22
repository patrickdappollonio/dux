import { useMemo, useState, type ReactNode } from "react"

import { useSearchPick } from "@/hooks/use-search-pick"

/** One row of a project list: a project, or an orphaned group of agents whose
 * project record is gone. */
export interface ProjectListRow {
  id: string
  name: string
  /** Trailing text on the first line (a count), or null for none. */
  label: string | null
  /** An optional second line under the name. */
  detail?: ReactNode
}

/** The filter and Enter-to-pick state of a project list. Without `onPick`,
 * Enter does nothing and a row click opens that row's menu. */
export function useProjectSearch(
  rows: ProjectListRow[],
  onPick: ((id: string) => void) | undefined,
) {
  const [query, setQuery] = useState("")
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (q === "") return rows
    return rows.filter((row) => row.name.toLowerCase().includes(q))
  }, [rows, query])
  const pick = useSearchPick({
    query,
    // A list without `onPick` has nothing for Enter to do.
    items: onPick ? filtered : [],
    keyOf: (row) => row.id,
    onPick: (row) => onPick?.(row.id),
  })
  return { query, setQuery, filtered, pick }
}

export type ProjectSearch = ReturnType<typeof useProjectSearch>
