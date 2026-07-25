import { FolderGit2, ExternalLink } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  closeFirstLoad,
  openAddProject,
  useDux,
  type FirstLoadDialogState,
} from "@/lib/store"
import type { Bootstrap } from "@/lib/bootstrapApi"

// THE first-load dialog: the first-run welcome AND the post-upgrade what's-new
// screen. ONE renderer, deliberately — the two screens share a frame and differ
// only in text and buttons, which is exactly how the approved design defines
// them, and one renderer is also what keeps desktop and mobile in step (the
// shared `GlobalOverlays` in App.tsx mounts this once for both shells).
//
// The content is server-projected plain prose (`dux_core::welcome_screen` and
// `dux_core::release_notes`), never Markdown, so there is no renderer here and
// no copy of the copy: the TUI says the same words from the same source.
//
// Every colour is a token (`--muted-foreground`, `--primary`, `--border`, …).
// The dux web palette is near-neutral greyscale with colour reserved for
// meaning, so this screen introduces no accent hue of its own.

/** The duck's column width and the art inside it, from the approved mock. */
const ART_COLUMN = "md:w-[152px]"
const DUCK_SIZE = "md:w-[118px]"

export function FirstLoadDialog() {
  const { firstLoad, bootstrap } = useDux()

  function handleOpenChange(open: boolean) {
    // Closing an AUTOMATIC screen dismisses it (the store posts the dismissal,
    // which records the version as seen in SQLite for the TUI too).
    if (!open) closeFirstLoad()
  }

  return (
    <Dialog open={firstLoad !== null} onOpenChange={handleOpenChange}>
      {/* 700px: comfortably wider than a routine dialog, because this one
          carries the duck column plus prose. On phones it becomes a bottom
          sheet — docked to the bottom edge, full width, square bottom corners —
          while staying the same component. The base primitive already caps the
          height to the visible viewport and scrolls internally. */}
      <DialogContent
        className="sm:max-w-[700px] max-md:top-auto max-md:bottom-0 max-md:left-0 max-md:max-w-none max-md:translate-x-0 max-md:translate-y-0 max-md:rounded-b-none"
      >
        {/* Guard the body on the state so nothing renders (and no stale content
            flashes) between closes. */}
        {firstLoad ? <Body state={firstLoad} bootstrap={bootstrap} /> : null}
      </DialogContent>
    </Dialog>
  )
}

function Body({
  state,
  bootstrap,
}: {
  state: FirstLoadDialogState
  bootstrap: Bootstrap | null
}) {
  const isWelcome = state.screen === "welcome"
  const website = bootstrap?.website_url ?? ""
  const notesUrl = state.notes?.html_url ?? ""
  // The footer names where the primary/link button will take you, so the
  // destination is visible before it is clicked (the same affordance the TUI
  // gallery puts in its footer).
  const destination = isWelcome ? website : notesUrl

  return (
    <>
      <div className="flex gap-4">
        {/* The duck, in its own column with a hairline divider. Dropped on
            phones: a duck plus a ribbon of text is worse than no duck. */}
        <div
          className={`hidden shrink-0 items-center justify-center border-r border-border md:flex ${ART_COLUMN}`}
        >
          <img
            src="/dux-logo.png"
            alt=""
            aria-hidden
            className={`object-contain ${DUCK_SIZE}`}
          />
        </div>

        <div className="flex min-w-0 flex-1 flex-col gap-3">
          {isWelcome ? (
            <WelcomeContent bootstrap={bootstrap} />
          ) : (
            <WhatsNewContent state={state} />
          )}
        </div>
      </div>

      {/* Misclick-safe spacing between the body and the buttons. */}
      <div className="h-2" />

      <DialogFooter className="sm:items-center sm:justify-between">
        {/* Muted, and on the opposite side from the buttons. `break-all` so a
            long release URL cannot widen the dialog. */}
        <p className="min-w-0 text-xs break-all text-muted-foreground/70">
          {destination}
        </p>
        <div className="flex flex-col-reverse gap-2 sm:flex-row">
          {isWelcome ? (
            <WelcomeButtons website={website} />
          ) : (
            <WhatsNewButtons notesUrl={notesUrl} />
          )}
        </div>
      </DialogFooter>
    </>
  )
}

// ── the welcome screen ───────────────────────────────────────────────────────

function WelcomeContent({ bootstrap }: { bootstrap: Bootstrap | null }) {
  const welcome = bootstrap?.welcome_screen
  return (
    <>
      <DialogHeader>
        <DialogTitle>Welcome to dux</DialogTitle>
        {welcome ? (
          <DialogDescription className="font-medium text-foreground">
            {welcome.tagline}
          </DialogDescription>
        ) : null}
      </DialogHeader>

      {welcome ? (
        <>
          {welcome.paragraphs.map((p, i) => (
            <p key={i} className="text-sm text-muted-foreground">
              {p}
            </p>
          ))}

          {/* The numbered steps DELIBERATELY repeat the prose above, so a reader
              who skips the paragraphs can still act. Numbered because it is a
              real sequence: no agent without a project, no launch without an
              agent. */}
          <ol className="mt-1 flex flex-col gap-3">
            {welcome.steps.map((step) => (
              <li key={step.number} className="flex gap-3">
                <span
                  aria-hidden
                  className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted text-xs font-medium text-foreground"
                >
                  {step.number}
                </span>
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className="text-sm font-medium text-foreground">
                    {step.title}
                  </span>
                  <span className="text-sm text-muted-foreground">
                    {step.detail}
                  </span>
                </span>
              </li>
            ))}
          </ol>
        </>
      ) : (
        // An older server that does not project the copy. Say so rather than
        // rendering an empty frame.
        <p className="text-sm text-muted-foreground">
          This server did not send the welcome text.
        </p>
      )}
    </>
  )
}

/**
 * A link-shaped button that is genuinely inert when it has no destination.
 *
 * MEASURED, not assumed: `<Button disabled render={<a href=… />} />` renders an
 * anchor that still carries its `href` and gains `disabled=""`/`data-disabled`.
 * Neither does anything to an anchor — the CSS `:disabled` pseudo-class (and so
 * the variant's `disabled:pointer-events-none`) does not match `<a>` — so it
 * stays clickable and navigates. When there is no URL we therefore render a real
 * disabled `<button>`, which is inert for real.
 *
 * Using the link DISMISSES the screen, matching the TUI, which closes the screen
 * and then opens the URL so the version is always recorded. Without this, a user
 * who clicks "Open full notes" or "Visit the website" and then closes the tab
 * records nothing and sees the same screen next launch. Base UI's render prop
 * merges this `onClick` onto the custom `<a>` (which carries none of its own), so
 * the handler runs before the browser's native new-tab navigation and the link
 * still opens. The sibling "Add a project" button does the same thing.
 */
function LinkButton({
  href,
  variant,
  children,
}: {
  href: string
  variant?: "outline"
  children: React.ReactNode
}) {
  if (href === "") {
    return (
      <Button variant={variant} className="max-md:w-full" disabled>
        {children}
      </Button>
    )
  }
  return (
    <Button
      variant={variant}
      className="max-md:w-full"
      onClick={() => closeFirstLoad()}
      render={
        <a href={href} target="_blank" rel="noopener noreferrer">
          {children}
        </a>
      }
    />
  )
}

function WelcomeButtons({ website }: { website: string }) {
  return (
    <>
      <LinkButton href={website} variant="outline">
        <ExternalLink data-icon="inline-start" />
        Visit the website
      </LinkButton>
      {/* The primary, filled action: the first thing a new user must do. Closing
          the dialog is what dismisses it. */}
      <Button
        className="max-md:w-full"
        onClick={() => {
          closeFirstLoad()
          openAddProject()
        }}
      >
        <FolderGit2 data-icon="inline-start" />
        Add a project
      </Button>
    </>
  )
}

// ── the what's-new screen ────────────────────────────────────────────────────

function WhatsNewContent({ state }: { state: FirstLoadDialogState }) {
  const notes = state.notes
  return (
    <>
      <DialogHeader>
        {/* The version chip. Muted, not accented: it is a label, not a state. */}
        <span className="flex items-center gap-2 text-xs text-muted-foreground">
          <span className="rounded-md bg-muted px-1.5 py-0.5 font-medium text-foreground">
            What&apos;s new in
          </span>
          <span className="font-mono">{notes?.version ?? ""}</span>
        </span>
        <DialogTitle>
          {notes?.headline || (state.loading ? "Loading…" : "Release notes")}
        </DialogTitle>
      </DialogHeader>

      {state.loading ? (
        <p className="text-sm text-muted-foreground">
          Fetching the release notes from GitHub…
        </p>
      ) : state.error !== null ? (
        // A real error in the body, not just a toast that may have auto-cleared.
        <p className="text-sm text-destructive">{state.error}</p>
      ) : notes ? (
        <>
          {notes.paragraphs.map((p, i) => (
            <p key={i} className="text-sm text-muted-foreground">
              {p}
            </p>
          ))}
          {notes.sections.length > 0 ? (
            <>
              <p className="mt-1 text-sm font-medium text-foreground">
                In this release
              </p>
              <ul className="flex flex-col gap-1.5">
                {notes.sections.map((s, i) => (
                  <li
                    key={i}
                    className="flex gap-2 text-sm text-muted-foreground"
                  >
                    <span aria-hidden className="text-muted-foreground/60">
                      –
                    </span>
                    <span className="min-w-0">{s}</span>
                  </li>
                ))}
              </ul>
            </>
          ) : null}
        </>
      ) : null}
    </>
  )
}

function WhatsNewButtons({ notesUrl }: { notesUrl: string }) {
  return (
    <>
      <Button
        variant="outline"
        className="max-md:w-full"
        onClick={() => closeFirstLoad()}
      >
        Close
      </Button>
      {/* Primary: the full notes on the release's own page. Genuinely inert
          while the notes (and therefore the link) are not in hand. */}
      <LinkButton href={notesUrl}>
        <ExternalLink data-icon="inline-start" />
        Open full notes
      </LinkButton>
    </>
  )
}
