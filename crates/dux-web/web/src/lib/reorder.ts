// Pure reorder helpers shared by the desktop sidebar and the mobile home screen. The DnD
// layer computes `activeId`/`overId`; these produce the id orders sent to the server,
// which validates them as strict permutations of the relevant set.

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
