// A drop, an image or long-text paste and the "Attach a file…" picker are one
// journey entered by three gestures, and this is where it lives.
//
// A drop saves the file and pastes its path, never its bytes: no agent CLI
// reads a file from its input stream, and injecting the text server-side would
// bypass the input-ownership gate on the socket, so the browser pastes the
// returned path over its own already-gated connection.
//
// Nothing here belongs to the terminal's lifetime, so it sits beside the pane
// rather than in the lifecycle: the drag handlers are rendered props, the
// picker is a hidden input, and the pane registers the clipboard listener.
import { useEffect, useRef } from "react"
import type { Terminal } from "@xterm/xterm"

import { clipboardPasteAction } from "@/lib/clipboardPaste"
import {
  type DropContext,
  type DropOutcome,
  dragCarriesFiles,
  dropToastFor,
  dragDropPasteFor,
  dropRefusalReason,
  nextFileDropToastId,
  pasteExceedsAttachmentLimit,
  pastePayload,
  tooLongToAttachReason,
} from "@/lib/fileDrop"
import { FileDropApiError, uploadDroppedFile } from "@/lib/fileDropApi"
import { notify, notifyBusy, notifyError } from "@/lib/notify"
import { registerAttachCapability } from "@/lib/attachRegistry"
import type { PtySocket } from "@/lib/ptySocket"

import type { LiveSettings } from "./liveValues"
import type { ConnectionIdentity, OwnershipVerdict } from "./channels"

// Where a saved file's path is written, and whether it can be written now. The
// upload loop is identical for a drop and a paste, so the sink is the only
// thing that differs and the only thing passed in; `activeUploadSink` picks
// one by the same question `focusTypingSurface` answers, which surface the
// user is typing into.
export type UploadSink = {
  /// Fills `DropContext.delivery`, which is the one word the toast changes.
  delivery: "sent" | "draft"
  /// Why the path cannot be delivered right now, in the words the
  /// stranded-file toast shows, or null when it can. Called IMMEDIATELY
  /// BEFORE each delivery, never once per drop: ownership can move and a
  /// socket can close between two files.
  unavailable: () => string | null
  deliver: (payload: string) => void
}

export type UploadPipelineDeps = {
  /// The pty this pane streams: the field the upload route stamps, and the key
  /// the attach capability is published under.
  id: string
  /// An agent pane and a terminal pane word their toast differently and resolve
  /// the paste form from different places, so the kind is a real parameter.
  kind: "agent" | "terminal"
  live: LiveSettings
  ownership: OwnershipVerdict
  connId: ConnectionIdentity
  termRef: { current: Terminal | null }
  ptyRef: { current: PtySocket | null }
  composeInputRef: { current: HTMLTextAreaElement | null }
  /// The draft splice, owned by the input surface: the compose sink delivers
  /// through it so a pasted path lands in the message the user is writing.
  insertComposeText: (text: string) => void
  /// Opens the hidden `<input type="file">`. Must be called straight from the
  /// activating click, or the browser's user activation is spent and no dialog
  /// appears.
  openFilePicker: () => Promise<File[]>
  /// Whether the pane is currently the input owner, and whether it is the
  /// mobile layout: both are RENDER values, because the drag handlers are
  /// rendered props rather than long-lived closures.
  isOwner: boolean
  isMobile: boolean
  fileDropEnabled: boolean
}

export type UploadPipeline = {
  /// Run a batch: the drop gesture's own entry point.
  runUpload: (
    files: File[],
    sink: UploadSink,
    pastedTextChars?: number,
  ) => Promise<void>
  /// Which surface a saved path should land in right now.
  activeUploadSink: () => UploadSink
  /// The picker gesture.
  attachFromPicker: () => void
  /// Arm the force-text-paste hatch, from the key handler.
  armForcedTextPaste: () => void
  /// The capture-phase paste listener's body.
  onClipboardPaste: (e: ClipboardEvent) => void
  /// May this pane act on this drag at all?
  paneAcceptsFileDrag: (e: React.DragEvent) => boolean
}

export function useUploadPipeline(deps: UploadPipelineDeps): UploadPipeline {
  const {
    id,
    kind,
    live,
    ownership,
    connId,
    termRef,
    ptyRef,
    composeInputRef,
    insertComposeText,
    openFilePicker,
    isOwner,
    isMobile,
    fileDropEnabled,
  } = deps
  // The `Ctrl+Shift+v` / `Cmd+Shift+v` text-paste hatch, armed by the key
  // handler and consumed by the `paste` listener the browser fires immediately
  // afterwards. A one-shot LATCH rather than a lasting preference: it describes
  // one keystroke.
  const forcedTextPasteRef = useRef(false)

  function terminalUploadSink(): UploadSink {
    return {
      delivery: "sent",
      unavailable: () => {
        if (!ownership.read()) return "another device took over input"
        // A write to a closed socket is dropped SILENTLY, so without this the
        // file would be reported as sent with nothing written.
        if (!termRef.current || !(ptyRef.current?.isOpen ?? false)) {
          return "the connection dropped"
        }
        return null
      },
      // xterm's own paste, so bracketed paste (DECSET 2004) is applied only
      // when the running program asked for it. Unlike the compose bar, which
      // refuses bracketed paste to keep a soft line break and a submitting
      // Enter distinct, a path contains neither.
      deliver: (payload) => termRef.current?.paste(payload),
    }
  }

  function composeUploadSink(): UploadSink {
    return {
      delivery: "draft",
      // No socket check: nothing goes on the wire until Send, which gates
      // itself in `sendCompose`. Ownership is still checked, so a demotion
      // mid-upload cannot stage input at a session this device no longer
      // drives, and so is the bar itself, which can go away mid-upload: the
      // draft survives that but the report would claim a message box that is
      // no longer on screen. Deliberately not a fallback to the terminal sink,
      // because the toast's wording was fixed when the sink was chosen and a
      // batch that changed destination halfway would misreport either side.
      unavailable: () => {
        if (!ownership.read()) return "another device took over input"
        if (!live.current.composeActive || composeInputRef.current === null) {
          return "the message box closed"
        }
        return null
      },
      deliver: insertComposeText,
    }
  }

  /// The surface a saved path should land in right now: the compose draft while
  /// the mobile compose bar is up, the terminal otherwise. Same rule, and the
  /// same refs, as `focusTypingSurface`.
  function activeUploadSink(): UploadSink {
    return live.current.composeActive && composeInputRef.current !== null
      ? composeUploadSink()
      : terminalUploadSink()
  }

  // Save each dropped or pasted file, then write its path to the sink.
  //
  // Sequential on purpose: outcomes and sent paths are both in dropped order,
  // which must not become whichever order the uploads finish in.
  //
  // The form each path takes and the length limit beside it are per-CLI (see
  // `pastePayload`), resolved immediately before each paste out of refs for the
  // same reason the ownership and socket checks are: a config reload or a
  // provider retarget can land between two files of one drop. A terminal runs a
  // shell, so its path is always quoted (see `TERMINAL_PASTE_FORM`) and neither
  // the setting nor the owning session's provider is consulted.
  //
  // `toastId` is this batch's own sonner id, minted by `runUpload`: two quick
  // drops sharing one id lose the first one's report under the second's spinner.
  async function handleUploadedFiles(
    files: File[],
    toastId: string,
    sink: UploadSink,
    pastedTextChars?: number,
  ) {
    if (files.length === 0) return
    const outcomes: DropOutcome[] = []

    for (const [i, file] of files.entries()) {
      // The drop overlay is already gone and an upload can wait a real amount
      // of time for a server-side slot, so without a spinner nothing visibly
      // happens. Same sonner id as this drop's final report, so the final
      // replaces the spinner rather than stacking a second toast.
      notifyBusy(
        files.length === 1
          ? `Uploading ${file.name}...`
          : `Uploading ${file.name} (${i + 1} of ${files.length})...`,
        // A fetch this tab is awaiting itself, not an engine status: if the
        // guard fires, the request is still in flight in this browser.
        { id: toastId, origin: "local" },
      )
      let saved
      try {
        saved = await uploadDroppedFile(file, {
          pty: id,
          // The TERMINAL SOCKET's id, not the events-socket one the other API
          // modules stamp in a header (the server refuses a PTY id there).
          conn: connId.read(),
        })
      } catch (e) {
        outcomes.push({
          kind: "refused",
          requestedName: file.name,
          // The STATUS decides the wording, not just the message: a 503 means
          // no upload slot came free and is worth retrying in a moment, which
          // is advice no other failure here deserves.
          reason:
            e instanceof FileDropApiError
              ? dropRefusalReason(e.status, e.message)
              : "the upload failed",
        })
        continue
      }
      // The folder travels with this file, not with the drop: a terminal's
      // directory changes on any `cd`, and these uploads are sequential, so two
      // files dropped together really can land in two folders.
      const where = {
        requestedName: saved.requested_name,
        savedName: saved.saved_name,
        path: saved.path,
        folderLabel: saved.folder_label,
      }

      // Asked IMMEDIATELY BEFORE this delivery, not once at the start of the
      // drop: ownership can move and the socket can close between two files.
      const unavailable = sink.unavailable()
      if (unavailable !== null) {
        outcomes.push({
          kind: "saved-not-sent",
          ...where,
          reason: unavailable,
        })
        continue
      }

      // Resolved per file, as above. The form and the CLI's character limit
      // come out together keyed by the same target, so neither is derived from
      // the other: a terminal has no limit whatever form it uses, and codex has
      // its limit on every form it can be configured with.
      const { form, charLimit } = dragDropPasteFor(
        live.current.configuredDropPaste,
        kind === "agent"
          ? {
              kind: "agent",
              launched: live.current.launchedDropPaste,
              provider: live.current.providerName,
            }
          : { kind: "terminal" },
      )
      const payload = pastePayload(where.path, form)
      // Too long for the receiving CLI to read as a path: codex files any paste
      // over its threshold away as generic large content before it tries to
      // recognize a path, so pasting would attach nothing. Reported as a
      // stranded file, with its full path, instead.
      if (charLimit !== null && pasteExceedsAttachmentLimit(payload, charLimit)) {
        outcomes.push({
          kind: "saved-not-sent",
          ...where,
          reason: tooLongToAttachReason(charLimit),
        })
        continue
      }

      sink.deliver(payload)
      // SENT, not "arrived". This is a socket write like any keystroke and
      // nothing acknowledges it: a take-over landing between the upload's
      // courtesy check and this frame reaching the server makes the server drop
      // it silently, so the toast claims only what dux knows.
      outcomes.push({ kind: "sent", ...where })
    }

    const ctx: DropContext = {
      kind: kind === "agent" ? "agent" : "terminal",
      delivery: sink.delivery,
      // Absent for every drop and for an image paste, so no existing report
      // gains a word; present only for a long text paste dux turned into a
      // document, where the user needs telling that it did.
      pastedTextChars,
    }
    const report = dropToastFor(outcomes, ctx)
    // Through the one raiser, so the configured dismiss window applies and the
    // spinner's leak guard is retired on the same id.
    //
    // Sticky when a file was saved but never delivered: the report then carries
    // the only copy on screen of a saved file's path, and the user must act
    // outside the toast to finish what they started.
    notify(report.tone, report.message, { id: toastId, sticky: report.sticky })
  }

  /// Raise the batch's spinner and make sure something final always replaces it.
  ///
  /// Per-file failures are already outcomes, so the only way out without a
  /// report is an unexpected throw, which would otherwise leave the spinner on
  /// screen until its leak guard expires. The id is minted here, once per
  /// batch, so a second drop cannot land its spinner on this drop's report.
  async function runUpload(
    files: File[],
    sink: UploadSink,
    pastedTextChars?: number,
  ) {
    const toastId = nextFileDropToastId()
    try {
      await handleUploadedFiles(files, toastId, sink, pastedTextChars)
    } catch (e) {
      notifyError(
        `The upload failed unexpectedly: ${e instanceof Error ? e.message : String(e)}`,
        { id: toastId },
      )
    }
  }

  /// The picker gesture: the entry a phone or keyboard-only user has, since a
  /// drag needs a desktop pointer and a paste needs the file on the clipboard.
  /// Everything after the files arrive is shared with the other two gestures.
  ///
  /// The sink is resolved after the picker settles, not before it opens: the
  /// dialog can sit open for a while, and where a path lands is a question
  /// about the moment of delivery. No `pastedTextChars`, which words the
  /// long-text paste toast and would describe a gesture that did not happen.
  function attachFromPicker(): void {
    void openFilePicker().then((files) => {
      if (files.length === 0) return
      void runUpload(files, activeUploadSink())
    })
  }

  // Published to the agent and terminal row menus while this pane is mounted
  // and owns the input. Ownership is part of the registration rather than
  // something the menu checks: a viewer's pane mounts completely, and an attach
  // from one would strand every file as saved-but-not-sent.
  useEffect(() => {
    if (!(isOwner && fileDropEnabled)) return
    return registerAttachCapability(id, attachFromPicker)
    // `attachFromPicker` is a component-body function reading only refs and
    // props, so a fresh identity every render says nothing new; listing it
    // would re-register on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, isOwner, fileDropEnabled])

  // An image on the clipboard, pasted: the same journey as a drop.
  //
  // The `paste` event rather than `navigator.clipboard.read()`, because dux is
  // routinely served over plain HTTP where the async Clipboard API's read is
  // blocked, while `clipboardData` needs no secure context.
  //
  // The listener is on the container in the capture phase, so dux sees a paste
  // before xterm's handler on the hidden textarea inside it. An image cancels
  // the event and stops propagation; anything else continues to xterm
  // untouched, which is the text path `attachCustomKeyEventHandler` keeps alive
  // by returning false without preventDefault for a paste chord.
  ///
  /// The text-paste hatch: `Ctrl+v` is image-wins, `Ctrl+Shift+v` (and
  /// `Cmd+Shift+v`) forces the text. A key event carries no clipboard contents
  /// and a paste event carries no modifiers, so the two halves meet through a
  /// latch. It expires on the task queue rather than waiting to be consumed: a
  /// chord that produces no paste event would otherwise leave it armed and
  /// disarm image handling for whatever pasted next.
  function armForcedTextPaste() {
    forcedTextPasteRef.current = true
    // The browser dispatches the native paste as the keydown's default action,
    // before yielding to the task queue, so this always lands after it.
    setTimeout(() => {
      forcedTextPasteRef.current = false
    }, 0)
  }

  function onClipboardPaste(e: ClipboardEvent) {
    const forceText = forcedTextPasteRef.current
    forcedTextPasteRef.current = false
    const items = Array.from(e.clipboardData?.items ?? [])
    const action = clipboardPasteAction(
      items,
      // Read synchronously: the decision must be made while the event is still
      // cancellable, and a `DataTransferItem` of kind `string` only yields its
      // contents through an async callback, by which time xterm has pasted.
      e.clipboardData?.getData("text/plain") ?? "",
      {
        uploadsEnabled: live.current.fileDropEnabled,
        isOwner: ownership.read(),
        forceText,
        // An AGENT gets the long-text threshold; a TERMINAL has none, and the
        // union is what makes that structural rather than a condition.
        pane:
          kind === "agent"
            ? { kind: "agent", longTextChars: live.current.pastedTextChars }
            : { kind: "terminal" },
      },
      new Date(),
    )
    if (action.kind === "upload") {
      e.preventDefault()
      e.stopPropagation()
      // Resolved HERE, at the gesture, so a paste into the compose box goes to
      // the draft and a paste into the terminal goes to the PTY.
      //
      // `pastedTextChars` is set only when these "files" are one long text
      // paste dux filed away, and it travels to the toast so the report can say
      // what happened rather than announcing a file the user never made.
      void runUpload(action.files, activeUploadSink(), action.pastedTextChars)
      return
    }
    if (action.kind === "refused") {
      // Cancel it too: a viewer's image paste must not fall through to xterm
      // (it would insert nothing, but silently), and the toast is the whole
      // point of refusing out loud rather than ignoring it.
      e.preventDefault()
      e.stopPropagation()
      notifyError(action.reason, {
        // One id per subject, not one for the whole listener: a refusal
        // replaces whatever holds its id, so an image refusal and a text
        // refusal sharing one would erase each other.
        id:
          action.subject === "text"
            ? "clipboard-text-paste"
            : "clipboard-image-paste",
      })
      return
    }
    // "xterm" and "ignore": touch nothing. Ordinary text paste is xterm's, and
    // an empty clipboard has nothing to do.
  }

  // A drag from a non-owner, on a phone, or while file drop is off is left
  // entirely alone: no overlay and no preventDefault.
  //
  // The server refuses every upload under `[server] file_drop_max_bytes = 0`
  // and stays the real enforcement; this gate stops a disabled feature from
  // advertising a drop target and refusing per file afterwards. It is closed
  // while the setting is merely unknown too (see `fileDropEnabled`).
  //
  // Deliberately not named `dragCarriesFiles`, which is the shared predicate in
  // `lib/fileDrop.ts` answering only whether a drag carries files; this answers
  // the wider question of whether this pane may act on it.
  function paneAcceptsFileDrag(e: React.DragEvent): boolean {
    return (
      fileDropEnabled &&
      isOwner &&
      !isMobile &&
      dragCarriesFiles(e.dataTransfer.types)
    )
  }

  return {
    runUpload,
    activeUploadSink,
    attachFromPicker,
    armForcedTextPaste,
    onClipboardPaste,
    paneAcceptsFileDrag,
  }
}
