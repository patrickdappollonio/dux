// THE UPLOAD PIPELINE: the three-gesture file journey.
//
// A DROP, an IMAGE (or long text) PASTE and the "Attach a file…" PICKER are the
// same journey entered by different gestures, and this is the one place it
// lives: the sinks that say where a saved path is written, the sequential batch
// loop that saves and delivers, the one toast per gesture, the clipboard
// routing that decides which gesture a paste even is, and the capability the
// row menus attach through.
//
// THE PREMISE, settled: a drop saves the file and pastes its PATH, never its
// bytes. No agent CLI reads a file from its input stream, and injecting the
// text server-side would bypass the input-ownership gate on the socket, so the
// browser pastes the returned path over its own already-gated connection.
//
// It is a pane-adjacent unit rather than part of the lifecycle because nothing
// in it belongs to the terminal's lifetime: the drag handlers are rendered
// props, the picker is a hidden input, and the clipboard listener is registered
// by the pane (on the container, in the capture phase) and by the compose
// textarea itself.
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

// WHERE a saved file's path is written, and whether it can be written right
// now. The upload loop below is identical for a drop and for a clipboard
// paste; the only thing that differs is this, so it is the only thing passed
// in. Two implementations exist (`terminalUploadSink`, `composeUploadSink`)
// and `activeUploadSink` picks between them exactly as `focusTypingSurface`
// picks a focus target, because the question is the same one: which surface
// is the user typing into.
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
      // xterm's own paste, which applies bracketed paste (DECSET 2004) when
      // the running program asked for it and sends plain text when it did not.
      // Building the bracket markers by hand here would be a second
      // implementation of something that already works.
      //
      // This deliberately differs from the compose bar, which refuses
      // bracketed paste. That rule exists because compose text has to keep a
      // soft line break and a submitting Enter distinct on the wire. A saved
      // file's path contains neither, so the reason does not apply here.
      deliver: (payload) => termRef.current?.paste(payload),
    }
  }

  function composeUploadSink(): UploadSink {
    return {
      delivery: "draft",
      // No socket check: nothing is going on the wire. The draft is text the
      // user reviews and then Sends, and `sendCompose` does its own gating at
      // that point. Ownership is still checked, because the compose bar only
      // exists for the input owner and a demotion mid-upload must not quietly
      // stage input at a session this device no longer drives.
      //
      // And the BAR ITSELF is checked, because it can go away mid-upload (a
      // rotation past the mobile breakpoint, `ui.compose_bar` switched off).
      // The draft state survives that, so the insert would still work; what
      // would not survive is the REPORT, which would say the path was added to
      // a message with no message box on screen to look at. Reporting the file
      // as saved-but-not-sent, with its full path, is the truthful outcome.
      // Deliberately not a fallback to the terminal sink: the toast's wording
      // was fixed when the sink was chosen at the gesture, and a batch that
      // quietly changed destination halfway would report the wrong one for
      // every file either side of the switch.
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
  // Sequential on purpose. The list of outcomes is in DROPPED order, and that is
  // also the order the paths are sent, which must not become whichever order the
  // uploads happen to finish in. One toast reports the whole drop at the end, so
  // a handful of files does not bury the screen.
  //
  // The FORM each path takes is per-CLI, because the agent CLIs do not agree on
  // how they read a pasted path (see `pastePayload`), and so is the length limit
  // beside it. Both come out of ONE resolved profile: what the focused tab's live
  // process launched with, off the spine (so a launch or a termination refreshes
  // it), falling back to what config says for its provider, off the bootstrap
  // document (so a `config.changed` refetch refreshes that).
  //
  // A TERMINAL is not a provider pane and never reads that setting: it runs a
  // SHELL, which is exactly why its path is always quoted rather than left bare
  // (see `TERMINAL_PASTE_FORM`). The owning session's provider is not consulted
  // either, for the separate reason that a companion terminal is not that agent.
  //
  // The form is resolved IMMEDIATELY BEFORE EACH PASTE, out of refs, for the same
  // reason the ownership and socket checks are: a drop's uploads are sequential,
  // so a config reload or a provider retarget can land between two files, and a
  // form snapshotted once at the top of the drop would silently outlive it.
  //
  // `toastId` is THIS batch's own sonner id, minted by `runUpload`. See
  // `nextFileDropToastId`: two quick drops sharing one id lose the first one's
  // report under the second one's spinner.
  async function handleUploadedFiles(
    files: File[],
    toastId: string,
    sink: UploadSink,
    pastedTextChars?: number,
  ) {
    if (files.length === 0) return
    const outcomes: DropOutcome[] = []

    for (const [i, file] of files.entries()) {
      // A spinner for THIS file, before the request goes out. The drop overlay
      // is already gone by now (`onDrop` clears it the moment the browser hands
      // the files over), and an upload can wait a bounded but real amount of
      // time for a server-side slot, so without this the interface returns to
      // normal and nothing visibly happens. Uploads are sequential, so a
      // multi-file drop counts through them rather than sitting on one message.
      //
      // Same sonner id as the report at the end of THIS drop, so the final
      // REPLACES the spinner in place rather than stacking a second toast, and
      // a concurrent drop cannot paint over either of them.
      notifyBusy(
        files.length === 1
          ? `Uploading ${file.name}...`
          : `Uploading ${file.name} (${i + 1} of ${files.length})...`,
        { id: toastId },
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
      // The folder travels with THIS file, not with the drop. A terminal's
      // directory changes the moment someone types `cd`, and these uploads are
      // sequential, so two files dropped together really can land in two
      // folders; keeping one label for the whole drop reported the last one for
      // all of them.
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

      // Resolved here, per file, rather than once per drop: see the note above.
      // The FORM and the CLI's character LIMIT come out together, keyed by the
      // same target, so neither can be derived from the other: a terminal is a
      // shell and has no limit whatever form it uses, and codex has its limit on
      // every form it can be configured with.
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
      // Too long for the receiving CLI to look at as a path. Codex files any
      // paste over its threshold away as generic large content before it tries
      // to recognize a path at all, so pasting this would put a placeholder in
      // the prompt and attach nothing, while the toast claimed success. Report
      // it as the stranded file it is: saved, here is the full path, go and
      // reference it yourself.
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
    // Through the ONE raiser, so the user's configured dismiss window applies.
    // A bare sonner call would silently use the library default. It also retires
    // the spinner's leak guard, since it lands on the same id.
    //
    // STICKY when a file was saved but never delivered. The report is then
    // carrying the full path of a file sitting on disk that the agent has not
    // been given, and that path exists nowhere else on screen: the user has to
    // act outside the toast (type the path, or drop the file again) to finish
    // what they started. A report that clears itself takes the only copy of
    // that information with it.
    notify(report.tone, report.message, { id: toastId, sticky: report.sticky })
  }

  /// Raise the batch's spinner and make sure something final always replaces it.
  /// Shared by the drop gesture and the clipboard paste, which differ only in
  /// the sink they hand over.
  ///
  /// The loop's per-file failures are already outcomes, so the only way out
  /// without a report is an unexpected throw. `handleUploadedFiles` is called
  /// with `void`, so that throw would become an unhandled rejection and leave
  /// the spinner on screen until its leak guard expires a minute later, still
  /// claiming the upload is running.
  ///
  /// The id is minted HERE, once per drop, and handed to both halves, so a
  /// second drop starting while this one is still uploading cannot land its
  /// spinner on this drop's report.
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

  /// THE PICKER GESTURE, the third way into the same journey.
  ///
  /// A drag needs a desktop pointer and a paste needs the file already on the
  /// clipboard; this needs neither, which is why it is the only entry point a
  /// phone or a keyboard-only desktop user has. Everything after the files
  /// arrive is shared with the other two gestures: the route, the destination
  /// (an agent's upload folder or a terminal's live directory), the naming, the
  /// per-provider path form, the length cap and the one toast.
  ///
  /// The sink is resolved AFTER the picker settles, not before it opens: the
  /// dialog can sit open for a while, and where a path should land is a
  /// question about the moment it is delivered (`activeUploadSink` reads the
  /// live compose state, and the sinks recheck ownership again per file).
  ///
  /// No `pastedTextChars`: that argument exists solely to word the long-text
  /// paste toast, and passing it here would make the report describe a gesture
  /// that did not happen.
  function attachFromPicker(): void {
    void openFilePicker().then((files) => {
      if (files.length === 0) return
      void runUpload(files, activeUploadSink())
    })
  }

  // Published to the agent and terminal ROW menus while this pane is mounted
  // and owns the input, so a desktop or keyboard-only user has a path into the
  // upload journey at all. Ownership is part of the registration rather than
  // something the menu checks: a viewer's pane mounts completely, and an
  // attach from one would strand every file as saved-but-not-sent. Uploads
  // being switched off retires it for the same reason the drag surface goes.
  useEffect(() => {
    if (!(isOwner && fileDropEnabled)) return
    return registerAttachCapability(id, attachFromPicker)
    // `attachFromPicker` is a component-body function reading only refs and
    // props, so a fresh identity every render says nothing new; listing it
    // would re-register on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, isOwner, fileDropEnabled])

  // An image on the clipboard, pasted. The same journey as a drop, entered by
  // the gesture people actually use: screenshot, paste, hand it to the agent.
  //
  // WHY THE `paste` EVENT AND NOT `navigator.clipboard.read()`. dux is
  // routinely served over plain HTTP on a Tailscale address, where the async
  // Clipboard API's read is blocked outright; the paste event's `clipboardData`
  // needs no secure context, because the user gesture IS the permission. Same
  // constraint, same answer as the Ctrl+v path below (and the CLAUDE.md
  // clipboard tenet).
  //
  // HOW THIS COEXISTS WITH THE Ctrl+v INTERCEPT, which is the fiddly part.
  // `attachCustomKeyEventHandler` deliberately returns false WITHOUT
  // preventDefault for a paste chord, precisely so the browser's native paste
  // event still fires and xterm's own handler reads the text out of
  // `clipboardData`. That is the text path and it must not change. So image
  // handling cannot live in the key handler at all (a key event carries no
  // clipboard contents); it lives in a `paste` listener registered on the
  // CONTAINER in the CAPTURE phase. Capture runs on ancestors before the
  // target, and xterm's handler is on the hidden textarea INSIDE the
  // container, so dux sees every paste first and can decide. For an image it
  // cancels the event and stops propagation, so xterm's handler never runs and
  // the browser inserts nothing; for anything else it does nothing whatsoever
  // and the event continues to xterm exactly as before. The image bytes never
  // reach xterm on either path.
  ///
  /// THE TEXT-PASTE HATCH. `Ctrl+v` is image-wins; `Ctrl+Shift+v` (and
  /// `Cmd+Shift+v`) forces the text. The key handler arms the latch and this
  /// consumes it, because a key event carries no clipboard contents and a paste
  /// event carries no modifiers, so the two halves of the gesture can only meet
  /// through a latch.
  ///
  /// Armed with a task-queue expiry rather than left to be consumed: a chord
  /// that produces no paste event at all (an empty clipboard on some browsers,
  /// a read the OS refuses) would otherwise leave the latch set and quietly
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
      // Read SYNCHRONOUSLY: the decision has to be made while the event is
      // still cancellable, and a `DataTransferItem` of kind `string` only
      // yields its contents through an async callback, by which time xterm has
      // already pasted. `getData` on the event's own `clipboardData` needs no
      // secure context, exactly like the image bytes beside it.
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
        // One id PER SUBJECT, not one for the whole listener. A refusal
        // replaces whatever is already on its id, so an image refusal and a
        // text refusal sharing one would erase each other: a viewer who pastes
        // a screenshot and then a wall of text would be told about exactly one
        // of them, with no way to know the other happened.
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

  // A drag from a non-owner, on a phone (where there is no drag), or while file
  // drop is switched off is left entirely alone: no overlay and no
  // preventDefault, so the browser does whatever it would normally do.
  //
  // `[server] file_drop_max_bytes = 0` is documented as switching file drop off,
  // and the server refuses every upload when it is. The server stays the real
  // enforcement; this gate is what stops a disabled feature ADVERTISING a drop
  // target, accepting the drop and only then reporting a refusal per file. It
  // is closed while the setting is merely UNKNOWN too, so nothing is offered
  // before dux can say the feature is there (see `fileDropEnabled`).
  //
  // Deliberately NOT called `dragCarriesFiles`: that name belongs to the one
  // shared predicate in `lib/fileDrop.ts`, which answers only "is this drag
  // carrying files", and the editor's file tree calls it under that name too.
  // This one answers the wider question ("and may this pane act on it"), so it
  // says so.
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
