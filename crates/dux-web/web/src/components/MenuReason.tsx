import { Menu as MenuPrimitive } from "@base-ui/react/menu"

import type { MenuReasonHandle } from "@/hooks/use-menu-reason"
import { renderProse } from "@/lib/prose"
import { cn } from "@/lib/utils"

// Why a menu item cannot be used, said in the menu itself, the one way every
// menu says it: in a dropdown and in a context menu alike, and on a phone inside
// the bottom sheet the menu becomes there. A disabled item takes no pointer
// events and a finger has no hover, so the sentence is written out rather than
// hung on the item as a tooltip.
//
// Where it sits follows what it covers. A reason shared by several items is
// said once, at the top of the menu, set apart from the rows by a separator; a
// reason only one item has is said right beneath that item, lined up with its
// words. Either way every item it greys names it as its description, so a
// screen reader hears why on the item itself.
//
// `useMenuReason` makes the reason; these pieces say it.
//
// The line is a menu group with only its label in it: the menu's own parts
// keep the popup's children to groups, items and separators, and an empty group
// is never stepped into by the arrow keys, so it is read once, not per item.

const LINE =
  "px-1.5 py-1 text-xs font-medium text-muted-foreground md:max-w-64"

function ReasonLine({ reason, beneathItem }: { reason: MenuReasonHandle; beneathItem: boolean }) {
  if (reason.text === null) return null
  return (
    <MenuPrimitive.Group data-slot="menu-reason-group">
      <MenuPrimitive.GroupLabel
        id={reason.id}
        data-slot="menu-reason"
        className={cn(LINE, beneathItem && "pl-7")}
      >
        {typeof reason.text === "string" ? reason.text : renderProse(reason.text)}
      </MenuPrimitive.GroupLabel>
    </MenuPrimitive.Group>
  )
}

/**
 * The reasons that cover several items, at the top of the menu, each once and
 * in the order given, then a separator. Nothing at all when none applies.
 */
export function MenuReasonsHead({ reasons }: { reasons: readonly MenuReasonHandle[] }) {
  const shown = reasons.filter((r) => r.text !== null)
  if (shown.length === 0) return null
  return (
    <>
      {shown.map((r) => (
        <ReasonLine key={r.id} reason={r} beneathItem={false} />
      ))}
      <MenuPrimitive.Separator
        data-slot="menu-reason-separator"
        className="-mx-1 my-1 h-px bg-border"
      />
    </>
  )
}

/** The reason only one item has, placed right after that item. */
export function MenuItemReason({ reason }: { reason: MenuReasonHandle }) {
  return <ReasonLine reason={reason} beneathItem />
}
