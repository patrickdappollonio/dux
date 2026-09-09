// Pure reorder helpers shared by the desktop sidebar and the mobile home screen. The DnD
// layer computes `activeId`/`overId`; these produce the id orders sent to the server,
// which validates them as strict permutations of the relevant set.

import type { ProjectView, SessionView } from "./types"

// arrayMove semantics: `activeId` relocated to the slot `overId` holds. A missing id, or
// both ids being the same, returns the original order unchanged.
export function moveItem(
  ids: string[],
  activeId: string,
  overId: string,
): string[] {
  if (activeId === overId) return ids
  const from = ids.indexOf(activeId)
  const to = ids.indexOf(overId)
  if (from === -1 || to === -1) return ids
  const next = ids.slice()
  next.splice(from, 1)
  next.splice(to, 0, activeId)
  return next
}

// Splice a reordered subgroup back into the full ordered project list: the server's
// `reorder_projects` requires the complete ordered set, while the UI only ever reorders
// within one visual group. Positions held by non-members keep their slot.
// `newGroupOrder` must be a permutation of the group's members; ids absent from
// `fullOrder` are ignored.
export function spliceGroupOrder(
  fullOrder: string[],
  groupMembers: string[],
  newGroupOrder: string[],
): string[] {
  const memberSet = new Set(groupMembers)
  // Only consider new-order ids that are actually members of the full list, so a
  // stale id can't inject a phantom entry.
  const queue = newGroupOrder.filter((id) => memberSet.has(id))
  let cursor = 0
  return fullOrder.map((id) => {
    if (!memberSet.has(id)) return id
    const replacement = queue[cursor] ?? id
    cursor += 1
    return replacement
  })
}

// The new complete project order after a drag within one group, whose members the caller
// passes in display order.
export function reorderProjectsInGroup(
  fullOrder: string[],
  groupMembers: string[],
  activeId: string,
  overId: string,
): string[] {
  const newGroupOrder = moveItem(groupMembers, activeId, overId)
  return spliceGroupOrder(fullOrder, groupMembers, newGroupOrder)
}

// Reorder `{ id }` items to match `orderedIds`. An item not named there keeps its original
// position, so a stale overlay degrades gracefully instead of dropping rows.
export function reorderById<T extends { id: string }>(
  items: T[],
  orderedIds: string[],
): T[] {
  const byId = new Map(items.map((item) => [item.id, item]))
  const named = new Set(orderedIds)
  const queue = orderedIds.filter((id) => byId.has(id))
  let cursor = 0
  return items.map((item) => {
    if (!named.has(item.id)) return item
    const next = byId.get(queue[cursor])
    cursor += 1
    return next ?? item
  })
}

// Whether two id orders are identical, which is what clears an optimistic overlay once a
// ViewModel arrives already matching it.
export function ordersMatch(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

// Apply the optimistic order overlays to the raw ViewModel arrays before they are
// partitioned, since display order is derived straight from array order. The overlays are
// independent: a session reorder touches one project's sessions, a project reorder the projects.
export function applyPendingOrders(
  projects: ProjectView[],
  sessions: SessionView[],
  pendingSessionOrder: { projectId: string; ids: string[] } | null,
  pendingProjectOrder: string[] | null,
): { projects: ProjectView[]; sessions: SessionView[] } {
  let nextProjects = projects
  let nextSessions = sessions

  if (pendingProjectOrder) {
    nextProjects = reorderById(projects, pendingProjectOrder)
  }

  if (pendingSessionOrder) {
    // `reorderById` leaves other projects' sessions in place, so the overlay's ids slot
    // into wherever this project's sessions already sit in the flat array.
    nextSessions = reorderById(sessions, pendingSessionOrder.ids)
  }

  return { projects: nextProjects, sessions: nextSessions }
}
