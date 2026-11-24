import { Check, FileType, RotateCw, SaveAll } from "lucide-react"

import { MenuItemReason, MenuReasonsHead } from "@/components/MenuReason"
import {
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu"
import type { MenuReasonHandle } from "@/hooks/use-menu-reason"
import {
  REOPEN_CHOICES,
  SAVE_CHOICES,
  sameSaveEncoding,
  type SaveEncoding,
} from "@/lib/editorEncoding"

// The rows of the editor status bar's encoding menu, which is the whole of
// that menu. Two submenus, not dialogs, so no trailing ellipsis. Any file can
// be reopened in any encoding: a garbled result is the person's call and a
// reopen away from undone, while a decode that would lose bytes is refused by
// the server. Save with is absent where there is no save (a file that can
// never be written, the large-file diff view).
//
// The busy reason greys every row this menu has, so it is the menu's head. The
// read-only reason greys "Save with encoding" alone, so it sits beneath that
// row. A row greyed by both names both lines.
export function EncodingMenuRows({
  busy,
  readOnly,
  fileBusy,
  writeBlocked,
  canSave,
  current,
  onReopen,
  onSave,
}: {
  busy: MenuReasonHandle
  readOnly: MenuReasonHandle
  fileBusy: boolean
  writeBlocked: boolean
  canSave: boolean
  current: SaveEncoding
  onReopen: (encoding: string) => void
  onSave: (choice: SaveEncoding) => void
}) {
  return (
    <>
      <MenuReasonsHead reasons={[busy]} />
      <DropdownMenuSub>
        {/* Not while this tab is saving: the save's answer is about the text
            it sent, and a reopen would put a different decoding under it. */}
        <DropdownMenuSubTrigger
          disabled={fileBusy}
          aria-describedby={busy.describedBy}
        >
          <RotateCw />
          Reopen with encoding
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent className="max-h-80 overflow-y-auto">
          {REOPEN_CHOICES.map((choice) => (
            <DropdownMenuItem
              key={choice.encoding}
              aria-current={
                choice.encoding === current.encoding ? "true" : undefined
              }
              onClick={() => onReopen(choice.encoding)}
            >
              <FileType />
              {choice.label}
              {choice.encoding === current.encoding && (
                <Check className="ml-auto" />
              )}
            </DropdownMenuItem>
          ))}
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      {canSave && (
        <DropdownMenuSub>
          <DropdownMenuSubTrigger
            disabled={fileBusy || writeBlocked}
            aria-describedby={
              [busy.describedBy, readOnly.describedBy]
                .filter(Boolean)
                .join(" ") || undefined
            }
          >
            <SaveAll />
            Save with encoding
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="max-h-80 overflow-y-auto">
            {SAVE_CHOICES.map((choice) => {
              const isCurrent = sameSaveEncoding(choice, current)
              return (
                <DropdownMenuItem
                  key={`${choice.encoding}:${choice.bom}`}
                  aria-current={isCurrent ? "true" : undefined}
                  // A file its encoding cannot write back unchanged can be
                  // converted, never saved again in that same encoding. The
                  // current encoding and mark is never a row to act on: it is
                  // not a conversion, and not an edit either, so a save from
                  // it would only rewrite untouched bytes (the line endings,
                  // say) for nothing.
                  disabled={isCurrent}
                  onClick={() =>
                    onSave({ encoding: choice.encoding, bom: choice.bom })
                  }
                >
                  <FileType />
                  {choice.label}
                  {isCurrent && <Check className="ml-auto" />}
                </DropdownMenuItem>
              )
            })}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      )}
      {canSave && <MenuItemReason reason={readOnly} />}
    </>
  )
}
