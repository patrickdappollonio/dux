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
import { useIsMobile } from "@/hooks/use-mobile"
import {
  closeFirstLoad,
  openAddProject,
  useDux,
  type FirstLoadDialogState,
} from "@/lib/store"
import type { Bootstrap, WelcomeScreenView } from "@/lib/bootstrapApi"
import { hasRenderableBody, NO_NOTES_EXPLANATION } from "@/lib/releaseNotes"

// The first-run welcome and the post-upgrade what's-new screen, from one
// renderer: they share a frame and differ only in text and buttons, and one
// renderer keeps desktop and mobile in step (`GlobalOverlays` mounts it once for
// both shells).
//
// The content is server-projected plain prose (`dux_core::welcome_screen` and
// `dux_core::release_notes`), never Markdown, so there is no renderer here and
// no second copy of the copy: the TUI says the same words from the same source.
//
// Every colour is a token, and this screen introduces no accent hue of its own.

/** The duck's column width and the art inside it, from the approved mock. */
const ART_COLUMN = "md:w-[152px]"
const DUCK_SIZE = "md:w-[118px]"

export function FirstLoadDialog() {
  const { firstLoad, bootstrap, standaloneEditor } = useDux()

  // The onboarding belongs to the main shell. The standalone editor tab is a
  // second SPA instance on the same store boot, so without this gate the screen
  // opens over a tab that is nothing but the editor; dismissal is recorded
  // server-side, so it still shows once, in the workspace tab.
  if (standaloneEditor) return null

  function handleOpenChange(open: boolean) {
    // Closing an AUTOMATIC screen dismisses it (the store posts the dismissal,
    // which records the version as seen in SQLite for the TUI too).
    if (!open) closeFirstLoad()
  }

  return (
    <Dialog open={firstLoad !== null} onOpenChange={handleOpenChange}>
      {/* Wider than a routine dialog, because this one carries the duck column
        * beside prose, and a bottom sheet on phones from the same component.
        * The base primitive caps the height to the viewport and would scroll the
        * whole popup, taking the title and buttons away with the prose, so on
        * phones its `grid`/`overflow-y-auto` are neutralized and the popup is a
        * flex column whose middle region is the only scroller. Every override
        * here is `max-md:`, so desktop is untouched. */}
      <DialogContent
        className="sm:max-w-[700px] max-md:top-auto max-md:bottom-0 max-md:left-0 max-md:flex max-md:max-w-none max-md:translate-x-0 max-md:translate-y-0 max-md:flex-col max-md:overflow-hidden max-md:rounded-b-none"
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
  // The footer names where the link button leads, so the destination is visible
  // before it is clicked, as the TUI gallery's footer does.
  const destination = isWelcome ? website : notesUrl
  // Width picks the layout, as a real DOM branch rather than a `hidden`/
  // `md:block` pair: the phone masthead and the desktop header are the same
  // pieces in different places, and only one may exist, or the dialog has two
  // accessible names and the version chip answers a query twice.
  const isMobile = useIsMobile()

  return (
    <>
      {isMobile ? (
        // Pinned, so the mark, the title and the version chip stay put while the
        // prose scrolls. `pr-8` clears the popup's own close button.
        <div
          data-slot="first-load-masthead"
          className="flex shrink-0 items-center gap-3 border-b border-border pr-8 pb-3"
        >
          <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
            <img
              src="/dux-logo.png"
              alt=""
              aria-hidden
              className="size-7 object-contain"
            />
          </span>
          {/* `leading-snug`: the base title is `leading-none`, which a wrapped
              release headline cannot survive on a narrow phone. */}
          <DialogTitle className="min-w-0 flex-1 leading-snug">
            {isWelcome ? WELCOME_TITLE : whatsNewTitle(state)}
          </DialogTitle>
          {isWelcome ? null : <VersionChip state={state} />}
        </div>
      ) : null}

      <div className="flex gap-4 max-md:min-h-0 max-md:flex-1">
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

        <div className="flex min-w-0 flex-1 flex-col gap-3 max-md:min-h-0">
          {isMobile ? null : isWelcome ? (
            <WelcomeHeader bootstrap={bootstrap} />
          ) : (
            <WhatsNewHeader state={state} />
          )}

          {/* The only scrolling region on phones. Inert on desktop, where the
            * popup itself scrolls and this is a plain wrapper. */}
          <div
            data-slot="first-load-body"
            className="flex flex-col gap-3 max-md:min-h-0 max-md:flex-1 max-md:overflow-y-auto"
          >
            {isWelcome ? (
              <WelcomeContent bootstrap={bootstrap} withTagline={isMobile} />
            ) : (
              <WhatsNewContent state={state} />
            )}
          </div>
        </div>
      </div>

      {/* Misclick-safe spacing between the body and the buttons. */}
      <div className="h-2 max-md:shrink-0" />

      <DialogFooter className="max-md:shrink-0 sm:items-center sm:justify-between">
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

const WELCOME_TITLE = "Welcome to dux"

function WelcomeHeader({ bootstrap }: { bootstrap: Bootstrap | null }) {
  const welcome = bootstrap?.welcome_screen
  return (
    <DialogHeader>
      <DialogTitle>{WELCOME_TITLE}</DialogTitle>
      {welcome ? <Tagline welcome={welcome} /> : null}
    </DialogHeader>
  )
}

function Tagline({ welcome }: { welcome: WelcomeScreenView }) {
  return (
    <DialogDescription className="font-medium text-foreground">
      {welcome.tagline}
    </DialogDescription>
  )
}

// `withTagline`: on phones the title lives in the pinned masthead and the
// tagline scrolls with the prose it introduces, so it is rendered here.
function WelcomeContent({
  bootstrap,
  withTagline,
}: {
  bootstrap: Bootstrap | null
  withTagline: boolean
}) {
  const welcome = bootstrap?.welcome_screen
  return (
    <>
      {welcome && withTagline ? <Tagline welcome={welcome} /> : null}

      {welcome ? (
        <>
          {welcome.paragraphs.map((p, i) => (
            <p key={i} className="text-sm text-muted-foreground">
              {p}
            </p>
          ))}

          {/* The steps deliberately repeat the prose above, so a reader who
            * skips the paragraphs can still act, and are numbered because the
            * sequence is real: no agent without a project. */}
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
 * A disabled `Button` rendered as an anchor keeps its `href` and gains only
 * `disabled` attributes, which do nothing to an `<a>`: the CSS `:disabled`
 * pseudo-class does not match one, so the variant's `pointer-events-none` never
 * applies and it still navigates. With no URL this renders a real `<button>`.
 *
 * Following the link dismisses the screen, matching the TUI, so the version is
 * recorded even if the user then closes the tab. The render prop merges this
 * `onClick` onto the `<a>`, which carries none of its own, so the handler runs
 * before the browser's navigation and the link still opens.
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

/** The headline, or what to say while it is not in hand. */
function whatsNewTitle(state: FirstLoadDialogState): string {
  return state.notes?.headline || (state.loading ? "Loading…" : "Release notes")
}

/** The version chip. Muted, not accented: it is a label, not a state. Above the
 *  title on desktop, at the masthead's trailing edge on phones. */
function VersionChip({ state }: { state: FirstLoadDialogState }) {
  return (
    <span className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
      <span className="rounded-md bg-muted px-1.5 py-0.5 font-medium text-foreground">
        What&apos;s new in
      </span>
      <span className="font-mono">{state.notes?.version ?? ""}</span>
    </span>
  )
}

function WhatsNewHeader({ state }: { state: FirstLoadDialogState }) {
  return (
    <DialogHeader>
      <VersionChip state={state} />
      <DialogTitle>{whatsNewTitle(state)}</DialogTitle>
    </DialogHeader>
  )
}

function WhatsNewContent({ state }: { state: FirstLoadDialogState }) {
  const notes = state.notes
  return (
    <>
      {state.loading ? (
        <p className="text-sm text-muted-foreground">
          Fetching the release notes from GitHub…
        </p>
      ) : state.error !== null ? (
        // A real error in the body, not just a toast that may have auto-cleared.
        <p className="text-sm text-destructive">{state.error}</p>
      ) : notes && !hasRenderableBody(notes) ? (
        // The release exists but its body held nothing the server-side parser
        // could read as prose or feature titles, which leaves the dialog a title,
        // two buttons and a blank middle. Routinely reachable, because the
        // generated release-body sections are not prose; CONTRIBUTING.md has the
        // format the parser needs.
        <p className="text-sm text-muted-foreground">{NO_NOTES_EXPLANATION}</p>
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
