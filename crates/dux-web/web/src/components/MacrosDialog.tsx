import { useState } from "react"
import { ListPlus, Pencil, Trash2 } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import {
  MACRO_SURFACE_OPTIONS,
  commitMacro,
  isMacroSurface,
  macroTextPreview,
  validateMacros,
} from "@/lib/macros"
import { closeMacrosDialog, saveMacros, useDux } from "@/lib/store"
import type { MacroSurface, MacroView } from "@/lib/types"

// The dialog's two internal modes: the LIST (rows + Add) or the per-row EDIT/ADD
// form. `editing` is the draft index being edited, "new" while adding, or null
// while on the list.
type EditorMode = { kind: "list" } | { kind: "form"; index: number | "new" }

const SURFACE_LABEL: Record<MacroSurface, string> = {
  agent: "Agent",
  terminal: "Terminal",
  both: "Both",
}

// The form body mounts only while the dialog is open and seeds its working copy
// from the store draft via a lazy initializer — no set-state-in-effect. The
// whole list is edited locally; Save sends it wholesale via `update_macros`.
function MacrosEditor({ initial }: { initial: MacroView[] }) {
  const [macros, setMacros] = useState<MacroView[]>(() =>
    initial.map((m) => ({ ...m })),
  )
  const [mode, setMode] = useState<EditorMode>({ kind: "list" })
  // The draft index pending delete confirmation, or null. Inline confirm row
  // (the dialog's established confirm style) rather than a nested modal.
  const [deleteIndex, setDeleteIndex] = useState<number | null>(null)

  if (mode.kind === "form") {
    return (
      <MacroForm
        macro={mode.index === "new" ? null : macros[mode.index]}
        onCancel={() => setMode({ kind: "list" })}
        onCommit={(macro) => {
          setMacros((prev) => commitMacro(prev, mode.index, macro))
          setMode({ kind: "list" })
        }}
      />
    )
  }

  const validationError = validateMacros(macros)

  return (
    <DialogContent showCloseButton={false} className="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Edit macros</DialogTitle>
        <DialogDescription>
          Text macros you can send to an agent or a terminal from the macro
          button on the terminal pane.
        </DialogDescription>
      </DialogHeader>

      {macros.length === 0 ? (
        <p className="py-6 text-center text-sm text-muted-foreground">
          No macros yet. Add one to get started.
        </p>
      ) : (
        <ul className="flex max-h-72 flex-col gap-1 overflow-y-auto">
          {macros.map((macro, index) => (
            <li
              key={index}
              className="flex flex-col gap-1.5 rounded-lg border p-2.5"
            >
              <div className="flex items-center gap-2">
                <span className="truncate font-medium">{macro.name}</span>
                <Badge variant="secondary" className="shrink-0">
                  {SURFACE_LABEL[macro.surface] ?? macro.surface}
                </Badge>
                <div className="ml-auto flex gap-1">
                  <SimpleTooltip content="Edit">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Edit ${macro.name}`}
                      onClick={() => {
                        setDeleteIndex(null)
                        setMode({ kind: "form", index })
                      }}
                    >
                      <Pencil />
                    </Button>
                  </SimpleTooltip>
                  <SimpleTooltip content="Delete">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Delete ${macro.name}`}
                      onClick={() => setDeleteIndex(index)}
                    >
                      <Trash2 />
                    </Button>
                  </SimpleTooltip>
                </div>
              </div>
              <p className="truncate font-mono text-xs text-muted-foreground">
                {macroTextPreview(macro.text)}
              </p>
              {deleteIndex === index ? (
                <div className="flex items-center gap-2 border-t pt-2">
                  <span className="text-xs text-destructive">
                    Delete this macro?
                  </span>
                  <div className="ml-auto flex gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      autoFocus
                      onClick={() => setDeleteIndex(null)}
                    >
                      Cancel
                    </Button>
                    <Button
                      variant="destructive"
                      size="sm"
                      onClick={() => {
                        setMacros((prev) => prev.filter((_, i) => i !== index))
                        setDeleteIndex(null)
                      }}
                    >
                      Delete
                    </Button>
                  </div>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}

      <Button
        variant="outline"
        size="sm"
        className="w-full"
        onClick={() => {
          setDeleteIndex(null)
          setMode({ kind: "form", index: "new" })
        }}
      >
        <ListPlus />
        Add macro
      </Button>

      {validationError ? (
        <p className="text-xs text-destructive">{validationError}</p>
      ) : null}

      {/* Misclick-safe spacing between the list/add controls and the footer. */}
      <div className="h-1" />
      <DialogFooter>
        <Button variant="outline" onClick={closeMacrosDialog}>
          Cancel
        </Button>
        <Button
          onClick={() => saveMacros(macros)}
          disabled={validationError !== null}
        >
          Save
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

// The add/edit form for a single macro. Local draft state seeded from the macro
// being edited (or blank for a new one) via lazy initializers; commits the row
// back to the list, which owns the wholesale set.
function MacroForm({
  macro,
  onCancel,
  onCommit,
}: {
  macro: MacroView | null
  onCancel: () => void
  onCommit: (macro: MacroView) => void
}) {
  const [name, setName] = useState(() => macro?.name ?? "")
  const [text, setText] = useState(() => macro?.text ?? "")
  const [surface, setSurface] = useState<MacroSurface>(
    () => macro?.surface ?? "agent",
  )

  const canSave = name.trim() !== "" && text !== ""

  function handleCommit() {
    if (!canSave) return
    onCommit({ name: name.trim(), text, surface })
  }

  return (
    <DialogContent showCloseButton={false} className="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>{macro ? "Edit macro" : "Add macro"}</DialogTitle>
      </DialogHeader>

      <div className="flex flex-col gap-3">
        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium text-muted-foreground">
            Name
          </label>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Review"
            autoFocus
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium text-muted-foreground">
            Text
          </label>
          <Textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="review this code for bugs"
            rows={6}
            className="font-mono"
          />
          <p className="text-xs text-muted-foreground">
            Newlines are sent as Alt+Enter — the whole macro is one prompt.
          </p>
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-sm font-medium text-muted-foreground">
            Surface
          </label>
          <Select
            value={surface}
            onValueChange={(v) => {
              if (typeof v === "string" && isMacroSurface(v)) setSurface(v)
            }}
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {MACRO_SURFACE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  <div className="flex flex-col">
                    <span>{option.label}</span>
                    <span className="text-xs text-muted-foreground">
                      {option.description}
                    </span>
                  </div>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      {/* Misclick-safe spacing between the fields and the footer. */}
      <div className="h-1" />
      <DialogFooter>
        <Button variant="outline" onClick={onCancel}>
          Cancel
        </Button>
        <Button onClick={handleCommit} disabled={!canSave}>
          {macro ? "Save macro" : "Add macro"}
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function MacrosDialog() {
  const { macrosDialogOpen, macrosDraft } = useDux()

  return (
    <Dialog
      open={macrosDialogOpen}
      onOpenChange={(o) => {
        if (!o) closeMacrosDialog()
      }}
    >
      {macrosDialogOpen && <MacrosEditor initial={macrosDraft} />}
    </Dialog>
  )
}
