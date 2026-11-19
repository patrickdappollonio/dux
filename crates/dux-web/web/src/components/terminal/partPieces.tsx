import { type ReactNode, type RefObject } from "react"
import { MonitorSmartphone } from "lucide-react"
import { AccessoryBar } from "@/components/AccessoryBar"
import { AGENT_PLACEHOLDER, ComposeBar, TERMINAL_PLACEHOLDER } from "@/components/ComposeBar"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { ESC, TAB } from "@/lib/termkeys"
import type { PtySocket } from "@/lib/ptySocket"
import { type AttachCover } from "@/lib/attachCover"
import { GlyphSpinner } from "@/components/GlyphSpinner"
import { plainBounce } from "@/components/terminal/plainBounce"
import { type InputSurface } from "@/components/terminal/inputSurface"
import { type UploadPipeline } from "@/components/terminal/uploadPipeline"
import type { TakeoverIntent } from "@/components/terminal/channels"
import type { TerminalPartProps } from "@/components/TerminalStack"
type TerminalPaneSurfaceProps = {
  /// The part's pty id, marked on its box so the stack can find it.
  partId: string
  /// The stack paints this part's take-over card over every part, so the part
  /// paints none of its own.
  cardInStack: boolean
  /// Off screen, the part keeps a real box the size of the stack's and is
  /// hidden, never shrunk.
  onScreen: boolean
  /// A press anywhere on the part makes it the typing target.
  onPress: () => void
  kind: TerminalPartProps["kind"]
  dragActive: boolean
  setDragActive: (active: boolean) => void
  dragDepthRef: RefObject<number>
  viewerOverflow: boolean
  hostRef: RefObject<HTMLDivElement | null>
  pointerTypeRef: RefObject<string>
  containerRef: RefObject<HTMLDivElement | null>
  pickerInput: ReactNode
  cover: AttachCover
  upload: UploadPipeline
  input: InputSurface
  providerName?: string
  ptyRef: RefObject<PtySocket | null>
  takeoverIntent: TakeoverIntent
  takeoverLabel: string | null
  ownerPresent: boolean
  takeOver: () => void
}

export function TerminalPaneSurface({
  partId,
  cardInStack,
  onScreen,
  onPress,
  kind,
  dragActive,
  setDragActive,
  dragDepthRef,
  viewerOverflow,
  hostRef,
  pointerTypeRef,
  containerRef,
  pickerInput,
  cover,
  upload,
  input,
  providerName,
  ptyRef,
  takeoverIntent,
  takeoverLabel,
  ownerPresent,
  takeOver,
}: TerminalPaneSurfaceProps) {
  // The pane's box is not itself the clip: the terminal, its covers and the
  // drop overlay are clipped by an inner layer, while the stack's overlay
  // layer sits over the stack's box beside it. (A dormant tab has no pane, and
  // its shell paints the overlay in the column instead.)
  return (
    <div
      className={
        onScreen
          ? "group relative min-h-0 w-full flex-1 bg-background"
          : "group invisible absolute inset-0 bg-background"
      }
      data-stack-part={partId}
      onPointerDownCapture={onPress}
      onDragEnter={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        event.preventDefault()
        dragDepthRef.current += 1
        setDragActive(true)
      }}
      onDragOver={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        // Browsers refuse a drop unless dragover cancels their default navigation.
        event.preventDefault()
        event.dataTransfer.dropEffect = "copy"
      }}
      onDragLeave={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        dragDepthRef.current = Math.max(0, dragDepthRef.current - 1)
        if (dragDepthRef.current === 0) setDragActive(false)
      }}
      onDrop={(event) => {
        if (!upload.paneAcceptsFileDrag(event)) return
        event.preventDefault()
        dragDepthRef.current = 0
        setDragActive(false)
        void upload.runUpload(
          Array.from(event.dataTransfer.files),
          upload.activeUploadSink(),
        )
      }}
    >
      <div className="absolute inset-0 overflow-hidden">
        {dragActive ? <FileDropOverlay kind={kind} /> : null}
        <div
          ref={hostRef}
          className={
            viewerOverflow ? "h-full w-full overflow-auto p-2" : "h-full w-full p-2"
          }
          onPointerDown={(event) => {
            pointerTypeRef.current = event.pointerType
          }}
          onContextMenu={(event) => {
            if (pointerTypeRef.current === "touch") {
              event.preventDefault()
              return
            }
            event.preventDefault()
            input.onRightClickPaste()
          }}
        >
          <div
            ref={containerRef}
            data-testid="terminal-container"
            className="h-full w-full [-webkit-touch-callout:none]"
          />
        </div>
        {pickerInput}
        <TerminalCover
          cover={cardInStack && cover.kind === "card" ? { kind: "none" } : cover}
          kind={kind}
          providerName={providerName}
          ptyRef={ptyRef}
          takeoverIntent={takeoverIntent}
          takeoverLabel={takeoverLabel}
          ownerPresent={ownerPresent}
          takeOver={takeOver}
        />
      </div>
    </div>
  )
}

function FileDropOverlay({ kind }: { kind: TerminalPartProps["kind"] }) {
  return (
    <div
      data-testid="file-drop-overlay"
      className="pointer-events-none absolute inset-2 z-20 flex flex-col items-center justify-center gap-1 rounded-lg border-2 border-dashed border-primary bg-background/90 p-4 text-center"
    >
      <p className="text-sm font-medium text-foreground">
        Drop to save the file and paste its path
      </p>
      <p className="text-xs text-muted-foreground">
        {kind === "agent"
          ? "It lands in this agent's upload folder, hidden from git and removed with the agent."
          : "It lands in the folder this terminal is currently in."}
      </p>
    </div>
  )
}

type TerminalCoverProps = {
  cover: AttachCover
  kind: TerminalPartProps["kind"]
  providerName?: string
  ptyRef: RefObject<PtySocket | null>
  takeoverIntent: TakeoverIntent
  takeoverLabel: string | null
  ownerPresent: boolean
  takeOver: () => void
}

export function TerminalCover(props: TerminalCoverProps) {
  switch (props.cover.kind) {
    case "none":
      return null
    case "box":
      return (
        <div className="absolute inset-0 z-20 flex items-center justify-center bg-background">
          <div className="flex items-center gap-3 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <span className="text-sm text-muted-foreground">
              {props.cover.reason === "lost"
                ? "Connection lost."
                : "Still waiting for the terminal's screen."}
            </span>
            <Button
              size="sm"
              variant="secondary"
              className="max-md:min-h-10 pointer-coarse:min-h-10"
              onClick={() =>
                plainBounce(props.ptyRef.current, props.takeoverIntent)
              }
            >
              Reconnect
            </Button>
          </div>
        </div>
      )
    case "spinner":
      return (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-card-foreground">
            <GlyphSpinner className="text-primary" />
            <span className="text-sm text-muted-foreground">
              {terminalCoverText(props.cover.wording, props.kind, props.providerName)}
            </span>
          </div>
        </div>
      )
    case "card":
      return (
        <TakeoverCard
          kind={props.kind}
          takeoverLabel={props.takeoverLabel}
          ownerPresent={props.ownerPresent}
          takeOver={props.takeOver}
        />
      )
  }
}

function terminalCoverText(
  wording: "starting" | "attaching" | "reconnecting",
  kind: TerminalPartProps["kind"],
  providerName?: string,
): string {
  if (wording === "reconnecting") return "Reconnecting…"
  if (wording === "attaching") return "Attaching…"
  return kind === "agent"
    ? "Starting " + (providerName ?? "agent") + "…"
    : "Launching terminal…"
}

function TakeoverCard({
  kind,
  takeoverLabel,
  ownerPresent,
  takeOver,
}: {
  kind: TerminalPartProps["kind"]
  takeoverLabel: string | null
  ownerPresent: boolean
  takeOver: () => void
}) {
  const title = takeoverLabel
    ? "Active on " + takeoverLabel
    : ownerPresent
      ? "Active on another device"
      : "Running in the background"
  return (
    <div className="absolute inset-0 z-20 flex items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm text-center">
        <CardHeader className="items-center gap-3">
          <MonitorSmartphone className="size-8 text-muted-foreground" />
          <CardTitle>{title}</CardTitle>
          <CardDescription>
            {ownerPresent ? (
              <>
                Only one device can type at a time. Take over to drive this{" "}
                {kind === "agent" ? "agent" : "terminal"} from here.
              </>
            ) : (
              <>
                The device driving this {kind === "agent" ? "agent" : "terminal"}{" "}
                disconnected, so we kept the{" "}
                {kind === "agent" ? "agent" : "terminal"} running in the
                background to avoid losing any progress. Take over to drive it
                from here.
              </>
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Button onClick={takeOver} className="w-full max-md:min-h-11 pointer-coarse:min-h-11">
            <MonitorSmartphone />
            Take over
          </Button>
        </CardContent>
      </Card>
    </div>
  )
}

type TerminalInputRowsProps = {
  accessoryBarShown: boolean
  composeBarShown: boolean
  input: InputSurface
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  kind: TerminalPartProps["kind"]
}

export function TerminalInputRows({
  accessoryBarShown,
  composeBarShown,
  input,
  composeInputRef,
  kind,
}: TerminalInputRowsProps) {
  return (
    <>
      {accessoryBarShown ? (
        <AccessoryBar
          onEsc={() => input.sendSeq(ESC)}
          onTab={() => input.sendSeq(TAB)}
          onNewline={input.sendNewline}
          onArrow={input.onArrow}
          onScroll={input.onScroll}
          ctrl={input.ctrl}
          alt={input.alt}
          onToggleCtrl={input.toggleCtrl}
          onToggleAlt={input.toggleAlt}
        />
      ) : null}
      {composeBarShown ? (
        <ComposeBar
          value={input.composeText}
          onChange={input.setComposeText}
          onSend={input.sendCompose}
          inputRef={composeInputRef}
          onForwardKey={input.sendSeq}
          placeholder={
            kind === "agent" ? AGENT_PLACEHOLDER : TERMINAL_PLACEHOLDER
          }
        />
      ) : null}
    </>
  )
}
