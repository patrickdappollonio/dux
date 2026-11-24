import { useLayoutEffect, type ReactNode } from "react"
import { Menu as MenuPrimitive } from "@base-ui/react/menu"

import { reasonWords, type MenuReasonHandle } from "@/hooks/use-menu-reason"
import { renderProse } from "@/lib/prose"
import { cn } from "@/lib/utils"

// Why a menu item cannot be used, said in the menu itself, the one way every
// menu says it: in a dropdown and in a context menu alike, and on a phone inside
// the bottom sheet the menu becomes there. A disabled item takes no pointer
// events and a finger has no hover, so the sentence is written out rather than
// hung on the item as a tooltip.
//
// Where it sits follows what it covers, so a reader never has to guess which
// items a sentence is about:
//
// - a reason for the whole menu (every item, or every item from the top down
//   to where its group ends) is one line at the top, set apart from the rows by
//   a separator: `MenuReasonsHead`;
// - a reason for a group of several items sits at the head of that group,
//   directly above the first item it greys, with no enabled item and no other
//   reason between: `MenuGroupReason`. Two different sentences each sit over
//   their own items;
// - a reason for one item sits right beneath that item, lined up with its
//   words: `MenuItemReason`.
//
// Every item a reason greys names it as its description, so a screen reader
// hears why on the item itself. The same sentence is said once however many
// handles carry it.
//
// `useMenuReason` makes the reason; these pieces say it. A handle's
// `describedBy` stays empty until a line that says it is drawn, so making a
// reason and never placing it leaves the items describing nothing rather than
// pointing at an id that does not exist.
//
// The line is a menu group with only its label in it: the menu's own parts
// keep the popup's children to groups, items and separators, and an empty group
// is never stepped into by the arrow keys, so it is read once, not per item.

const LINE =
  "px-1.5 py-1 text-xs font-medium text-muted-foreground md:max-w-64"

function ReasonLine({ reason, beneathItem }: { reason: MenuReasonHandle; beneathItem: boolean }) {
  const { drawnAs, id } = reason
  useLayoutEffect(() => drawnAs(id), [drawnAs, id])
  if (reason.text === null) return null
  return (
    <MenuPrimitive.Group data-slot="menu-reason-group">
      <MenuPrimitive.GroupLabel
        id={id}
        data-slot="menu-reason"
        className={cn(LINE, beneathItem && "pl-7")}
      >
        {typeof reason.text === "string" ? reason.text : renderProse(reason.text)}
      </MenuPrimitive.GroupLabel>
    </MenuPrimitive.Group>
  )
}

// A handle whose sentence another handle already says: it draws nothing and
// names that line instead.
function EchoOf({ reason, line }: { reason: MenuReasonHandle; line: MenuReasonHandle }) {
  const { drawnAs } = reason
  const lineId = line.id
  useLayoutEffect(() => drawnAs(lineId), [drawnAs, lineId])
  return null
}

function Lines({ reasons }: { reasons: readonly MenuReasonHandle[] }) {
  const said = new Map<string, MenuReasonHandle>()
  const parts: ReactNode[] = []
  for (const reason of reasons) {
    if (reason.text === null) continue
    const words = reasonWords(reason.text)
    const first = said.get(words)
    if (first === undefined) {
      said.set(words, reason)
      parts.push(<ReasonLine key={reason.id} reason={reason} beneathItem={false} />)
    } else if (first.id !== reason.id) {
      parts.push(<EchoOf key={reason.id} reason={reason} line={first} />)
    }
  }
  return parts.length === 0 ? null : <>{parts}</>
}

const anyShown = (reasons: readonly MenuReasonHandle[]) => reasons.some((r) => r.text !== null)

function ReasonSeparator() {
  return (
    <MenuPrimitive.Separator
      data-slot="menu-reason-separator"
      className="-mx-1 my-1 h-px bg-border"
    />
  )
}

/**
 * The reasons that cover the whole menu, or everything from its top down to
 * where their group ends: each said once, in the order given, then a
 * separator. Nothing at all when none applies.
 */
export function MenuReasonsHead({ reasons }: { reasons: readonly MenuReasonHandle[] }) {
  if (!anyShown(reasons)) return null
  return (
    <>
      <Lines reasons={reasons} />
      <ReasonSeparator />
    </>
  )
}

/**
 * The reasons that cover a group of several items, placed directly above the
 * first item they grey. `apart` puts a separator before the line, for a group
 * that follows other rows; leave it off when the line is first in the menu, so
 * the menu never starts on a rule. Nothing at all when none applies.
 */
export function MenuGroupReason({
  reasons,
  apart = false,
}: {
  reasons: readonly MenuReasonHandle[]
  apart?: boolean
}) {
  if (!anyShown(reasons)) return null
  return (
    <>
      {apart ? <ReasonSeparator /> : null}
      <Lines reasons={reasons} />
    </>
  )
}

/** The reason only one item has, placed right after that item. */
export function MenuItemReason({ reason }: { reason: MenuReasonHandle }) {
  return <ReasonLine reason={reason} beneathItem />
}
