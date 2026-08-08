import { useState } from "react"
import type { CSSProperties } from "react"
import {
  DndContext,
  MouseSensor,
  TouchSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core"
import type { DragEndEvent } from "@dnd-kit/core"
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable"
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
  MOUSE_DRAG_ACTIVATION,
  TOUCH_DRAG_ACTIVATION,
} from "@/lib/dragActivation"
import {
  MACRO_SURFACE_OPTIONS,
  commitMacro,
  isMacroSurface,
  macroDragIds,
  macroTextPreview,
  reorderMacrosByDrag,
  validateMacros,
} from "@/lib/macros"
import {
  closeMacrosDialog,
  persistMacroOrder,
  saveMacros,
  useDux,
} from "@/lib/store"
import type { MacroSurface, MacroView } from "@/lib/types"
import { cn } from "@/lib/utils"

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
  // The bootstrap document holds the authoritative macro list. Until it loads,
  // the draft was seeded empty, so a wholesale save would wipe the server's
  // macros — disable Save in that window (the store also refuses it defensively).
  const { bootstrap } = useDux()
  const [macros, setMacros] = useState<MacroView[]>(() =>
    initial.map((m) => ({ ...m })),
  )
  const [mode, setMode] = useState<EditorMode>({ kind: "list" })
  // The draft index pending delete confirmation, or null. Inline confirm row
  // (the dialog's established confirm style) rather than a nested modal.
  const [deleteIndex, setDeleteIndex] = useState<number | null>(null)

  // Reorder by drag, the agents-pane idiom: mouse drags on a 6px pull (a plain
  // click on the row's buttons stays a click); touch drags on a HOLD, so the
  // gesture coexists with the list's own scroll. Values and reasoning live in
  // lib/dragActivation.ts.
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION }),
    useSensor(TouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION }),
  )

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over) return
    const prev = macros
    const next = reorderMacrosByDrag(prev, String(active.id), String(over.id))
    if (next === prev) return
    // Rows are about to shift index; a pending delete-confirm would point at
    // the wrong row afterwards, so retire it with the drop.
    setDeleteIndex(null)
    // Optimistic apply, then persist through the same wholesale save as the
    // Save button. When the draft can't be saved yet (validation error, or the
    // bootstrap window where a wholesale PUT would wipe the server's macros),
    // the order stays in the draft and the eventual Save carries it.
    setMacros(next)
    if (bootstrap === null || validateMacros(next) !== null) return
    void persistMacroOrder(next).then((ok) => {
      if (ok) return
      // Snap back — but only if nothing else edited the draft while the save
      // was in flight; a functional update makes the check race-free.
      setMacros((current) => (current === next ? prev : current))
    })
  }

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
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={macroDragIds(macros)}
            strategy={verticalListSortingStrategy}
          >
            <ul className="flex max-h-72 flex-col gap-1 overflow-y-auto">
              {macros.map((macro, index) => (
                <SortableMacroRow
                  key={index}
                  macro={macro}
                  index={index}
                  deleteOpen={deleteIndex === index}
                  onEdit={() => {
                    setDeleteIndex(null)
                    setMode({ kind: "form", index })
                  }}
                  onAskDelete={() => setDeleteIndex(index)}
                  onCancelDelete={() => setDeleteIndex(null)}
                  onConfirmDelete={() => {
                    setMacros((prev) => prev.filter((_, i) => i !== index))
                    setDeleteIndex(null)
                  }}
                />
              ))}
            </ul>
          </SortableContext>
        </DndContext>
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
          disabled={validationError !== null || bootstrap === null}
        >
          Save
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

// One draggable macro row. Whole-row drag, exactly like the agents-pane rows:
// `useSortable` supplies the drag props (spread on the row itself, no separate
// handle), the sensors' activation gates are what let the row's Edit/Delete
// buttons keep working (a plain click never moves 6px; a touch tap never holds
// 300ms), and the wrapper carries the Y-locked transform so the row can't
// slide out of the dialog column sideways.
function SortableMacroRow({
  macro,
  index,
  deleteOpen,
  onEdit,
  onAskDelete,
  onCancelDelete,
  onConfirmDelete,
}: {
  macro: MacroView
  index: number
  deleteOpen: boolean
  onEdit: () => void
  onAskDelete: () => void
  onCancelDelete: () => void
  onConfirmDelete: () => void
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: `macro-${index}` })
  const style: CSSProperties = {
    // Vertical reorder list: lock the drag to the Y axis (see AgentFlatRow).
    transform: transform
      ? `translate3d(0, ${Math.round(transform.y)}px, 0)`
      : undefined,
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }

  return (
    // While dragging, the row visibly LIFTS (shadow + stacking) so a touch
    // hold that armed the drag reads as "grabbed" before the finger moves.
    // The wrapper (the `li`) carries ref/transform/lift and keeps its list
    // semantics; the drag props go on the row BODY (dnd-kit's attributes set
    // `role="button"`/`tabIndex`, which belong on the activator, not the
    // listitem) — the same wrapper/activator split as the sidebar rows. The
    // delete-confirm strip stays OUTSIDE the activator so its buttons are
    // never contended by a drag.
    <li
      ref={setNodeRef}
      style={style}
      className={cn(
        "flex flex-col gap-1.5 rounded-lg border bg-background p-2.5",
        isDragging && "z-10 shadow-lg",
      )}
    >
      <div
        {...attributes}
        {...listeners}
        className="flex touch-manipulation flex-col gap-1.5"
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
                onClick={onEdit}
              >
                <Pencil />
              </Button>
            </SimpleTooltip>
            <SimpleTooltip content="Delete">
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Delete ${macro.name}`}
                onClick={onAskDelete}
              >
                <Trash2 />
              </Button>
            </SimpleTooltip>
          </div>
        </div>
        <p className="truncate font-mono text-xs text-muted-foreground">
          {macroTextPreview(macro.text)}
        </p>
      </div>
      {deleteOpen ? (
        <div className="flex items-center gap-2 border-t pt-2">
          <span className="text-xs text-destructive">Delete this macro?</span>
          <div className="ml-auto flex gap-2">
            <Button variant="outline" size="sm" autoFocus onClick={onCancelDelete}>
              Cancel
            </Button>
            <Button variant="destructive" size="sm" onClick={onConfirmDelete}>
              Delete
            </Button>
          </div>
        </div>
      ) : null}
    </li>
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
