// Pure reorder helpers shared by the desktop sidebar and the mobile home screen.
// Kept free of React and dnd-kit so they're trivially unit-testable: the DnD
// layer only computes `activeId`/`overId`, then these functions produce the new
// id orders that get sent to the server (which validates them as strict
// permutations of the relevant set).

import type { ProjectView, SessionView } from "./types"

// arrayMove semantics: return a new array with `activeId` relocated to the slot
// currently occupied by `overId`. If either id is missing, or they're the same,
// the original order is returned unchanged (a no-op drag).
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

// Splice a reordered subgroup back into the full ordered project list. The
// server's `reorder_projects` requires the COMPLETE ordered set of every
// project id (with and without agents). The UI, however, only ever reorders
// WITHIN one visual group (the "with agents" list or the "no agents" list). This
// walks the full list and, at each position originally held by a member of the
// reordered group, drops in the next id from `newGroupOrder`. Positions held by
// non-group members keep their existing slot. The result is the full list with
// just that group's internal order rewritten.
//
// `newGroupOrder` must be a permutation of the group's members as they appear in
// `fullOrder`; ids in `newGroupOrder` that aren't in `fullOrder` are ignored.
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
    // Replace this group slot with the next id from the reordered group.
    const replacement = queue[cursor] ?? id
    cursor += 1
    return replacement
  })
}

// Compute the new COMPLETE project order after dragging within one group. The
// caller knows which group was dragged (its members in display order) and the
// drag's active/over ids; this reorders that group via `moveItem` and splices
// the result back into the full list.
export function reorderProjectsInGroup(
  fullOrder: string[],
  groupMembers: string[],
  activeId: string,
  overId: string,
): string[] {
  const newGroupOrder = moveItem(groupMembers, activeId, overId)
  return spliceGroupOrder(fullOrder, groupMembers, newGroupOrder)
}

// Reorder an array of `{ id }` items to match `orderedIds`. Items whose id is in
// `orderedIds` are emitted in that order; any item NOT named in `orderedIds`
// keeps its original relative position, occupying the slots between named items.
// This makes a stale overlay (one that doesn't mention a freshly added item)
// degrade gracefully instead of dropping rows.
export function reorderById<T extends { id: string }>(
  items: T[],
  orderedIds: string[],
): T[] {
  const byId = new Map(items.map((item) => [item.id, item]))
  const named = new Set(orderedIds)
  const queue = orderedIds.filter((id) => byId.has(id))
  let cursor = 0
  return items.map((item) => {
    // Slots originally held by a named item are refilled, in order, from the
    // overlay; unnamed items pass through untouched.
    if (!named.has(item.id)) return item
    const next = byId.get(queue[cursor])
    cursor += 1
    return next ?? item
  })
}

// Whether two id orders are identical (same length, same ids, same positions).
// Used to clear an optimistic overlay once a ViewModel arrives whose order
// already matches what we optimistically applied.
export function ordersMatch(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

// Apply the optimistic order overlays to the raw ViewModel arrays before they're
// partitioned for display. Returns reordered `projects`/`sessions` so the
// existing `partitionProjects` pipeline (which derives display order straight
// from array order) renders the in-flight order without any further changes.
// Both overlays are independent: a session reorder only touches one project's
// sessions; a project reorder only touches the project array.
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
    // Reorder only the target project's sessions; everything else stays put.
    // `reorderById` keeps non-named sessions (other projects') in place, so the
    // overlay's ids — which are exactly this project's sessions — slot in
    // wherever those sessions already sit in the flat array.
    nextSessions = reorderById(sessions, pendingSessionOrder.ids)
  }

  return { projects: nextProjects, sessions: nextSessions }
}
