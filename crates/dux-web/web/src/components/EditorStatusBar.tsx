import { useEffect, useState } from "react"
import { ChevronDown, Code2, FileType } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { StartTruncatedText } from "@/components/StartTruncatedText"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  cursorLabel,
  formatFileSize,
  indentationLabel,
  lineCountLabel,
  readEditorStatus,
  type EditorCursorStatus,
  type EditorStatusSource,
  type StatusBarPlan,
} from "@/lib/editorStatus"
import { cn } from "@/lib/utils"
import { copyToClipboard } from "@/lib/clipboard"
import { notifyError, notifySuccess } from "@/lib/notify"
import { chip, prose } from "@/lib/prose"

interface Subscription {
  dispose(): void
}

// The editor events the bar follows, on top of what it reads. Structural, like
// `EditorStatusSource`, so Monaco's real editor fits it and a test can fake it.
export interface StatusBarEditor extends EditorStatusSource {
  onDidChangeCursorSelection(listener: () => void): Subscription
  onDidChangeModelContent(listener: () => void): Subscription
  onDidChangeModel(listener: () => void): Subscription
  onDidChangeModelOptions(listener: () => void): Subscription
}

// The live half, kept in this component rather than in the editor body, so a
// keystroke or a cursor move re-renders one thin bar and not the whole editor.
function useEditorCursorStatus(
  editor: StatusBarEditor | null,
): EditorCursorStatus | null {
  // Tagged with the editor it was read from, so a reading left over from a
  // previous editor (the last file, before the pane swapped) is never painted
  // for the new one while its first read is still to come.
  const [reading, setReading] = useState<{
    editor: StatusBarEditor
    status: EditorCursorStatus | null
  } | null>(null)
  useEffect(() => {
    if (editor === null) return
    const refresh = () => setReading({ editor, status: readEditorStatus(editor) })
    refresh()
    const subscriptions = [
      editor.onDidChangeCursorSelection(refresh),
      editor.onDidChangeModelContent(refresh),
      // A path switch swaps the model under the same editor.
      editor.onDidChangeModel(refresh),
      // Indentation detection lands as an options change.
      editor.onDidChangeModelOptions(refresh),
    ]
    return () => {
      for (const s of subscriptions) s.dispose()
    }
  }, [editor])
  return editor !== null && reading?.editor === editor ? reading.status : null
}

// The open file's path, which copies itself when pressed. It ellipsizes at the
// start, so the file's name always shows. The copy is confirmed in a toast:
// nothing on screen changes when the clipboard does.
//
// Its neighbours are the dirty dot and the read-only label, both plain text,
// so under a fine pointer it keeps the bar's own height; a finger gets the
// 40px floor on both axes (the bar grows to match on the vertical one).
function PathCopyButton({ path }: { path: string }) {
  function copy(): void {
    void copyToClipboard(path).then((ok) => {
      if (ok) notifySuccess(prose`Copied ${chip(path)} to the clipboard.`)
      else
        notifyError(
          prose`Couldn't copy ${chip(path)}: the browser refused the clipboard.`,
        )
    })
  }
  return (
    <SimpleTooltip content="Copy path">
      <button
        type="button"
        onClick={copy}
        aria-label={`Copy path ${path}`}
        className="flex h-full min-w-0 items-center rounded-sm text-left outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:min-w-10"
      >
        <StartTruncatedText
          text={path}
          className="min-w-0 font-mono text-foreground"
        />
      </button>
    </SimpleTooltip>
  )
}

// Hidden on a phone, where the bar keeps only the path, the dirty dot, the
// read-only label, an image's dimensions and the encoding selector: the facts
// a phone user acts on, and the one control that has no other home there.
const DESKTOP_ONLY = "max-md:hidden"

export function EditorStatusBar({
  plan,
  editor,
  languageLabel,
  languageItems,
  encodingItems = null,
}: {
  plan: StatusBarPlan
  editor: StatusBarEditor | null
  languageLabel: string
  // The picker's rows, shared with the phone fold's submenu so the two
  // cannot drift.
  languageItems: React.ReactNode
  // The encoding menu's rows (reopen, save with). Shown while the plan names
  // an encoding, on every screen: this selector is their only home.
  encodingItems?: React.ReactNode
}) {
  const status = useEditorCursorStatus(plan.editing ? editor : null)
  return (
    // One fixed height whatever it carries, so the content above it never
    // jumps as items come and go. A finger, at any width, gets the 40px
    // floor: the encoding selector is on the bar on a phone too.
    <div className="flex h-6.5 shrink-0 items-center gap-3 border-t px-3 text-xs text-muted-foreground pointer-coarse:h-10">
      {plan.path !== null && (
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <PathCopyButton path={plan.path} />
          {plan.dirty && (
            <>
              <SimpleTooltip content="Unsaved changes">
                <span className="shrink-0 text-primary" aria-hidden="true">
                  ●
                </span>
              </SimpleTooltip>
              <span className="sr-only">unsaved changes</span>
            </>
          )}
          {plan.readOnly && (
            <SimpleTooltip
              content={
                plan.readOnlyReason ??
                "This file is read-only: it is a symlink to an external file or a .git path"
              }
            >
              <span className="shrink-0">read-only</span>
            </SimpleTooltip>
          )}
        </div>
      )}
      <div className="ml-auto flex shrink-0 items-center gap-3">
        {status !== null && (
          <>
            <span className={cn("tabular-nums", DESKTOP_ONLY)}>
              {cursorLabel(status)}
            </span>
            <span className={cn("tabular-nums", DESKTOP_ONLY)}>
              {lineCountLabel(status.lineCount)}
            </span>
            <span className={DESKTOP_ONLY}>
              {indentationLabel(status.indentation)}
            </span>
            {plan.mixedEol ? (
              <SimpleTooltip content={`Mixed line endings: a save writes every one as ${status.eol}`}>
                <span className={DESKTOP_ONLY}>Mixed</span>
              </SimpleTooltip>
            ) : (
              <span className={DESKTOP_ONLY}>{status.eol}</span>
            )}
          </>
        )}
        {plan.dimensions !== null && (
          <span className="tabular-nums">
            {plan.dimensions.width} × {plan.dimensions.height}
          </span>
        )}
        {plan.size !== null && (
          <SimpleTooltip content="Size on disk when last read or saved">
            <span className={cn("tabular-nums", DESKTOP_ONLY)}>
              {formatFileSize(plan.size)}
            </span>
          </SimpleTooltip>
        )}
        {plan.encoding !== null && (
          // On every screen, unlike the language picker: on a phone this is
          // the bar's one control and the encoding menu's only home.
          <span className="inline-flex">
            <DropdownMenu>
              <SimpleTooltip content={`File encoding: ${plan.encoding}`}>
                <DropdownMenuTrigger
                  render={
                    // Under the 40px floor on the vertical axis for a fine
                    // pointer only: its neighbours on this bar are plain
                    // text. A finger at any width gets the floor (the bar
                    // grows to match), and the label keeps it wider than
                    // 40px on the other axis.
                    <Button
                      variant="ghost"
                      size="xs"
                      className="h-5.5 font-normal text-muted-foreground pointer-coarse:h-10"
                      aria-label={`File encoding: ${plan.encoding}`}
                    />
                  }
                >
                  <FileType />
                  {plan.encoding}
                  <ChevronDown />
                </DropdownMenuTrigger>
              </SimpleTooltip>
              <DropdownMenuContent align="end" side="top">
                {encodingItems}
              </DropdownMenuContent>
            </DropdownMenu>
          </span>
        )}
        {plan.language && (
          <span className={cn("inline-flex", DESKTOP_ONLY)}>
            <DropdownMenu>
              <SimpleTooltip content={`Syntax language: ${languageLabel}`}>
                <DropdownMenuTrigger
                  render={
                    // Ghost and under the 40px floor on the vertical axis for
                    // a fine pointer only: its neighbours on this bar are
                    // plain text, and a finger at desktop width gets the
                    // floor (the bar grows to match).
                    <Button
                      variant="ghost"
                      size="xs"
                      className="h-5.5 font-normal text-muted-foreground pointer-coarse:h-10"
                      aria-label={`Syntax language: ${languageLabel}`}
                    />
                  }
                >
                  <Code2 />
                  {languageLabel}
                  <ChevronDown />
                </DropdownMenuTrigger>
              </SimpleTooltip>
              <DropdownMenuContent align="end" side="top">
                {languageItems}
              </DropdownMenuContent>
            </DropdownMenu>
          </span>
        )}
      </div>
    </div>
  )
}
