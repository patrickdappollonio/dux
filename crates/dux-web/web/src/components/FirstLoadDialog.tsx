import { FolderGit2, ExternalLink } from "lucide-react"

import {
  Modal,
  useHeldWhileClosed,
  useModalLifetime,
  type ModalAction,
} from "@/components/modal"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  closeFirstLoad,
  openAddProject,
  useDux,
  type FirstLoadDialogState,
} from "@/lib/store"
import type { WelcomeScreenView } from "@/lib/bootstrapApi"
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
const ART_COLUMN = "w-[152px]"
const DUCK_SIZE = "w-[118px]"

export function FirstLoadDialog() {
  const { firstLoad, standaloneEditor } = useDux()
  // The onboarding belongs to the main shell. The standalone editor tab is a
  // second SPA instance on the same store boot, so without this gate the screen
  // opens over a tab that is nothing but the editor; dismissal is recorded
  // server-side, so it still shows once, in the workspace tab.
  const open = firstLoad !== null && !standaloneEditor
  const life = useModalLifetime(open)
  return life.mounted ? <FirstLoad key={life.epoch} open={open} onClosed={life.onClosed} /> : null
}

function FirstLoad({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { firstLoad, bootstrap } = useDux()
  // The screen as it was when the close began, kept while the close animates.
  const state = useHeldWhileClosed(open, firstLoad)
  // The phone's title row carries the dux mark, where a computer has the
  // duck's column beside the prose.
  const phone = useIsMobile()
  if (state === null) return null
  const isWelcome = state.screen === "welcome"
  const welcome = bootstrap?.welcome_screen
  const website = bootstrap?.website_url ?? ""
  const notesUrl = state.notes?.html_url ?? ""
  const title = isWelcome ? WELCOME_TITLE : whatsNewTitle(state)

  const actions: ModalAction[] = isWelcome
    ? [
        {
          label: "Visit the website",
          tone: "plain",
          icon: ExternalLink,
          // Following the link dismisses the screen, matching the TUI, so the
          // version is recorded even if the user then closes the tab.
          ...linkOrReason(website, "This server did not send the website's address."),
        },
        {
          // The first thing a new user must do. Closing is what dismisses it.
          label: "Add a project",
          tone: "primary",
          icon: FolderGit2,
          onSelect: () => {
            closeFirstLoad()
            openAddProject()
          },
        },
      ]
    : [
        // The approved order: Close, then the primary that ends the row.
        { label: "Close", tone: "plain", onSelect: () => closeFirstLoad() },
        {
          label: "Open full notes",
          tone: "primary",
          icon: ExternalLink,
          ...linkOrReason(
            notesUrl,
            state.loading
              ? "The release notes are still loading."
              : "These release notes have no page to open.",
          ),
        },
      ]

  return (
    <Modal
      kind="report"
      open={open}
      // Closing an AUTOMATIC screen dismisses it (the store posts the dismissal,
      // which records the version as seen in SQLite for the TUI too).
      onClose={() => closeFirstLoad()}
      onClosed={onClosed}
      // A screen that opened by itself records itself as seen when it closes, so
      // the browser's Back must not be what closes it. Opened from the menu, it
      // is an ordinary dialog and Back closes it.
      historyEntry={state.automatic !== true}
      // The approved design: a corner close and only the screen's own buttons
      // in the footer, where a report would otherwise end with a Close.
      closeAt="corner"
      size="w700"
      title={
        phone ? (
          <span data-slot="first-load-masthead" className="flex items-center gap-3">
            <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
              <img src="/dux-logo.png" alt="" aria-hidden className="size-7 object-contain" />
            </span>
            <span className="min-w-0">{title}</span>
          </span>
        ) : (
          title
        )
      }
      description={
        isWelcome ? (
          welcome ? (
            <span className="font-medium text-foreground">{welcome.tagline}</span>
          ) : undefined
        ) : (
          <VersionChip state={state} />
        )
      }
      actions={actions}
      // Names where the link button leads, so the destination is visible before
      // it is clicked, as the TUI gallery's footer does. `break-all` so a long
      // release URL cannot widen the dialog.
      footerNote={
        (isWelcome ? website : notesUrl) === "" ? undefined : (
          <span className="break-all text-muted-foreground/70">{isWelcome ? website : notesUrl}</span>
        )
      }
    >
      <div className="flex gap-4">
        {/* The duck, in its own column with a hairline divider. Dropped on
            phones: a duck plus a ribbon of text is worse than no duck. */}
        {phone ? null : (
          <div
            className={`flex shrink-0 items-center justify-center border-r border-border ${ART_COLUMN}`}
          >
            <img src="/dux-logo.png" alt="" aria-hidden className={`object-contain ${DUCK_SIZE}`} />
          </div>
        )}
        <div data-slot="first-load-body" className="flex min-w-0 flex-1 flex-col gap-3">
          {isWelcome ? <WelcomeContent welcome={welcome} /> : <WhatsNewContent state={state} />}
        </div>
      </div>
    </Modal>
  )
}

// A link while there is somewhere to go, and otherwise a button that takes no
// press and says why: an anchor cannot be disabled.
function linkOrReason(
  href: string,
  reason: string,
): { href: string; onSelect: () => void } | { onSelect: () => void; disabledReason: string } {
  return href === ""
    ? { onSelect: () => {}, disabledReason: reason }
    : { href, onSelect: () => closeFirstLoad() }
}

// ── the welcome screen ───────────────────────────────────────────────────────

const WELCOME_TITLE = "Welcome to dux"

function WelcomeContent({ welcome }: { welcome: WelcomeScreenView | undefined }) {
  if (!welcome) {
    // An older server that does not project the copy. Say so rather than
    // rendering an empty frame.
    return (
      <p className="text-sm text-muted-foreground">
        This server did not send the welcome text.
      </p>
    )
  }
  return (
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
  )
}

// ── the what's-new screen ────────────────────────────────────────────────────

/** The headline, or what to say while it is not in hand. */
function whatsNewTitle(state: FirstLoadDialogState): string {
  return state.notes?.headline || (state.loading ? "Loading…" : "Release notes")
}

/** The version chip, under the title. Muted, not accented: it is a label, not
 *  a state. */
function VersionChip({ state }: { state: FirstLoadDialogState }) {
  return (
    <span className="flex items-center gap-2 text-xs text-muted-foreground">
      <span className="rounded-md bg-muted px-1.5 py-0.5 font-medium text-foreground">
        What&apos;s new in
      </span>
      <span className="font-mono">{state.notes?.version ?? ""}</span>
    </span>
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
