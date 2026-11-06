import { useEffect, useState, type CSSProperties, type ReactNode } from "react"
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
import {
  ArrowDown,
  ArrowDownWideNarrow,
  ArrowUp,
  ArrowUpToLine,
  Check,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  ExternalLink,
  Eye,
  EyeOff,
  GitBranch,
  GitPullRequest,
  GripVertical,
  Plus,
  Unlink,
} from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { InlineCode } from "@/components/ui/inline-code"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useIsMobile } from "@/hooks/use-mobile"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { sessionLabel } from "@/lib/agentWorkspace"
import { MOUSE_DRAG_ACTIVATION, TOUCH_DRAG_ACTIVATION } from "@/lib/dragActivation"
import {
  prActivity,
  prIconClass,
  prKeyOf,
  prName,
  prOrigin,
  prRowAction,
  prStateLabel,
  samePrKey,
  sessionPrs,
} from "@/lib/pr"
import { moveItem } from "@/lib/reorder"
import { sessionsApi } from "@/lib/sessionsApi"
import {
  closePullRequests,
  detachPullRequest,
  hidePullRequest,
  movePullRequest,
  openAttachPullRequest,
  reorderPullRequests,
  setPullRequestSort,
  showPullRequest,
  useDux,
} from "@/lib/store"
import { ALWAYS_REVEALED_ON_TOUCH } from "@/lib/touchReveal"
import type { PrBranchesView, PrKeyView, PrView, SessionView } from "@/lib/types"
import { cn } from "@/lib/utils"

// The header's one height token: the view switch and the Sort control sit
// side by side at it, lifted to the touch floor on a coarse pointer.
const HEADER_HEIGHT = "h-9 pointer-coarse:h-10"
// A row's own controls (the grip, the `⋯`, Show again): compact beside a
// mouse, the 40px floor under a finger.
const ROW_CONTROL = "size-8 pointer-coarse:size-10"
// A view tab fills its list beside a mouse and takes the 40px floor under a
// finger. The selected one is a filled segment in the foreground tone, which
// the primitive's own faint tint does not read as on this dialog's surface.
const VIEW_TRIGGER =
  "pointer-coarse:h-10 data-active:bg-foreground data-active:text-background dark:data-active:bg-foreground dark:data-active:text-background data-active:hover:text-background dark:data-active:hover:text-background"

type View = "prs" | "branches"

// Every pull request an agent has, in two views: the list in the agent's order
// with a way to hide, detach, reorder and show again, and the branch log that
// says which branch each was found on. A dialog on a computer and a bottom
// sheet on a phone, around one body. Keyed to its agent, so it closes itself
// when the agent leaves the spine.
export function PullRequestsDialog() {
  const { pullRequestsTarget, spine, bootstrap } = useDux()
  const isMobile = useIsMobile()
  const session = spine?.sessions.find((s) => s.id === pullRequestsTarget)
  const open = useVanishedTargetGuard(
    pullRequestsTarget !== null,
    session !== undefined,
    closePullRequests,
  )
  const onOpenChange = (o: boolean) => {
    if (!o) closePullRequests()
  }
  const body =
    open && session ? (
      <PullRequestsBody
        session={session}
        phone={isMobile}
        canAttach={bootstrap?.gh_available ?? false}
      />
    ) : null
  if (isMobile) {
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent
          side="bottom"
          className="max-h-[85dvh] gap-0 overflow-y-auto overscroll-contain rounded-t-2xl pb-[max(env(safe-area-inset-bottom),0.75rem)]"
        >
          {body}
        </SheetContent>
      </Sheet>
    )
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false} className="gap-3 sm:max-w-[640px]">
        {body}
      </DialogContent>
    </Dialog>
  )
}

function PullRequestsBody({
  session,
  phone,
  canAttach,
}: {
  session: SessionView
  phone: boolean
  canAttach: boolean
}) {
  const [view, setView] = useState<View>("prs")
  const name = sessionLabel(session)
  const visible = sessionPrs(session)
  const hidden = session.prs_hidden ?? []
  const manual = session.pr_sort === "manual"
  const branches = useBranches(session.id, session.pr_revision ?? 0)
  const Title = phone ? SheetTitle : DialogTitle
  const Description = phone ? SheetDescription : DialogDescription

  return (
    <div className={cn("flex min-h-0 flex-col gap-3", phone && "p-4")}>
      <div className={cn("flex flex-col gap-1", phone && "pr-10")}>
        <Title>Pull requests</Title>
        <Description>
          {view === "prs" ? (
            <>
              Found on the branches <InlineCode>{name}</InlineCode> has been on,
              plus the ones you attached.{" "}
              {manual
                ? "In your order; the first one is what the banner shows. New ones found later go after yours."
                : "Open ones come first, newest first, and the first one is what the banner shows."}
            </>
          ) : (
            <>
              Every branch <InlineCode>{name}</InlineCode> has had checked out
              in its worktree, newest first, with the pull requests found for
              each.
            </>
          )}
        </Description>
      </div>

      <div className="flex items-center gap-2">
        <Tabs
          value={view}
          onValueChange={(v) => setView(v as View)}
          className="min-w-0 flex-1"
        >
          {/* Under a finger the list drops its inset so each trigger reaches
              the 40px floor while the list stays at Sort's height. */}
          <TabsList className="w-full group-data-horizontal/tabs:h-9 pointer-coarse:p-0 pointer-coarse:group-data-horizontal/tabs:h-10">
            <TabsTrigger value="prs" className={VIEW_TRIGGER}>
              <GitPullRequest />
              Pull requests
              <span className="tabular-nums opacity-70">{visible.length}</span>
            </TabsTrigger>
            <TabsTrigger value="branches" className={VIEW_TRIGGER}>
              <GitBranch />
              Branches
              {branches.data ? (
                <span className="tabular-nums opacity-70">
                  {branches.data.branches.length}
                </span>
              ) : null}
            </TabsTrigger>
          </TabsList>
        </Tabs>
        <SortControl sessionId={session.id} manual={manual} phone={phone} />
      </div>

      {view === "prs" ? (
        <PullRequestsView
          session={session}
          name={name}
          visible={visible}
          hidden={hidden}
        />
      ) : (
        <BranchesView
          prs={[...visible, ...hidden]}
          hidden={hidden}
          branches={branches}
        />
      )}

      <div
        className={cn(
          "-mx-4 -mb-4 flex items-center gap-2 border-t bg-muted/50 p-4",
          phone && "mx-0 mb-0 rounded-none bg-transparent px-0 pb-0",
        )}
      >
        {canAttach ? (
          <Button
            variant="outline"
            className={cn(HEADER_HEIGHT, phone && "flex-1")}
            onClick={() => openAttachPullRequest(session.id)}
          >
            <Plus />
            Attach pull request…
          </Button>
        ) : (
          <p className="text-sm text-muted-foreground">
            Attaching needs a signed-in gh.
          </p>
        )}
        {phone ? null : (
          <Button
            variant="outline"
            className={cn("ml-auto", HEADER_HEIGHT)}
            onClick={closePullRequests}
          >
            Close
          </Button>
        )}
      </div>
    </div>
  )
}

// The order control, modelled on the agents pane's Sort: a static trigger,
// with the active mode marked in its menu.
// On a phone the word goes, so the row fits the narrowest sheet beside the two
// view tabs, whose counts are data and stay.
function SortControl({
  sessionId,
  manual,
  phone,
}: {
  sessionId: string
  manual: boolean
  phone: boolean
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="outline"
            aria-label="Sort pull requests"
            className={cn("shrink-0", HEADER_HEIGHT)}
          />
        }
      >
        <ArrowDownWideNarrow />
        {phone ? null : "Sort"}
        <ChevronDown className="opacity-60" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => setPullRequestSort(sessionId, "automatic")}>
          {manual ? <span className="size-4" /> : <Check />}
          Automatic
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => setPullRequestSort(sessionId, "manual")}>
          {manual ? <Check /> : <span className="size-4" />}
          Manual
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function keyId(key: PrKeyView): string {
  return `${key.host}/${key.owner_repo}#${key.number}`
}

function PullRequestsView({
  session,
  name,
  visible,
  hidden,
}: {
  session: SessionView
  name: string
  visible: PrView[]
  hidden: PrView[]
}) {
  // A drop is shown at once, ahead of the server: the order it left, tied to
  // the revision it was made against. A new revision means the server has
  // answered (or something else changed the list), and its order wins again.
  const [dropped, setDropped] = useState<{ revision: number; ids: string[] } | null>(
    null,
  )
  const revision = session.pr_revision ?? 0
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION }),
    useSensor(TouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION }),
  )
  const keyed = visible.flatMap((pr) => {
    const key = prKeyOf(pr)
    return key ? [{ pr, key, id: keyId(key) }] : []
  })
  const serverIds = keyed.map((row) => row.id)
  const ids =
    dropped && dropped.revision === revision && dropped.ids.length === serverIds.length
      ? dropped.ids
      : serverIds
  const byId = new Map(keyed.map((row) => [row.id, row]))
  const rows = ids.flatMap((id) => {
    const row = byId.get(id)
    return row ? [row] : []
  })

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over) return
    const next = moveItem(ids, String(active.id), String(over.id))
    if (next === ids) return
    setDropped({ revision, ids: next })
    const order = next.flatMap((id) => {
      const row = byId.get(id)
      return row ? [row.key] : []
    })
    void reorderPullRequests(session.id, order).then((ok) => {
      if (!ok) setDropped(null)
    })
  }

  if (visible.length === 0 && hidden.length === 0) {
    return (
      <p className="py-6 text-center text-sm text-muted-foreground">
        No pull requests yet. dux looks on every branch{" "}
        <InlineCode>{name}</InlineCode> has been on, every few minutes.
      </p>
    )
  }

  return (
    <div className="flex max-h-[50dvh] flex-col gap-1 overflow-y-auto">
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext items={ids} strategy={verticalListSortingStrategy}>
          <ul className="flex flex-col gap-1">
            {rows.map((row, index) => (
              <SortablePrRow
                key={row.id}
                id={row.id}
                pr={row.pr}
                sessionId={session.id}
                prKey={row.key}
                first={index === 0}
                last={index === rows.length - 1}
              />
            ))}
          </ul>
        </SortableContext>
      </DndContext>
      {hidden.length > 0 ? (
        <HiddenSection sessionId={session.id} hidden={hidden} />
      ) : null}
    </div>
  )
}

// A small, muted label beside a name (Attached, Current, a count): it says
// something about the row without competing with the title for attention.
const QUIET_TAG =
  "inline-flex h-4 shrink-0 items-center rounded-sm bg-muted px-1.5 text-[11px] leading-none font-normal text-muted-foreground tabular-nums"

function PrRowText({ pr, muted = false }: { pr: PrView; muted?: boolean }) {
  const tone = prIconClass(pr.state)
  const origin = prOrigin(pr)
  const activity = prActivity(pr)
  return (
    <>
      <GitPullRequest className={cn("size-4 shrink-0", tone, muted && "opacity-60")} />
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-center gap-2">
          <span className={cn("shrink-0 font-semibold", tone, muted && "opacity-60")}>
            {prName(pr)}
          </span>
          <span className={cn("min-w-0 truncate", muted && "text-muted-foreground")}>
            {pr.title}
          </span>
          {(pr.attached ?? pr.overridden) ? (
            // On a phone the details already say "attached by you" when that
            // is all it is, and the title needs the width more.
            <span className={cn(QUIET_TAG, origin?.kind === "attached" && "max-md:hidden")}>
              Attached
            </span>
          ) : null}
        </span>
        {/* One line, whatever the width: the branch name is what gives way,
          * and each separator travels with the detail after it so none is
          * ever left at a line's end. */}
        <span
          data-slot="pr-details"
          className={cn(
            "flex min-w-0 items-center gap-1.5 text-xs whitespace-nowrap text-muted-foreground",
            muted && "opacity-80",
          )}
        >
          <span className={cn("shrink-0", tone)}>{prStateLabel(pr.state)}</span>
          {origin?.kind === "branch" ? (
            <Detail shrink>
              <GitBranch className="size-3 shrink-0" />
              <span className="truncate">{origin.name}</span>
            </Detail>
          ) : origin?.kind === "attached" ? (
            <Detail>attached by you</Detail>
          ) : null}
          {activity ? <Detail>{activity}</Detail> : null}
        </span>
      </span>
    </>
  )
}

function Detail({ children, shrink = false }: { children: ReactNode; shrink?: boolean }) {
  return (
    <span className={cn("inline-flex items-center gap-1.5", shrink ? "min-w-0" : "shrink-0")}>
      <span aria-hidden>•</span>
      <span className={cn("inline-flex items-center gap-1", shrink && "min-w-0")}>{children}</span>
    </span>
  )
}

// One draggable row. The drag starts on the grip alone, so the row's text can
// be selected and its `⋯` pressed without arming it.
function SortablePrRow({
  id,
  pr,
  prKey,
  sessionId,
  first,
  last,
}: {
  id: string
  pr: PrView
  prKey: PrKeyView
  sessionId: string
  first: boolean
  last: boolean
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id })
  const reducedMotion = usePrefersReducedMotion()
  const style: CSSProperties = {
    // Vertical reorder list: lock the drag to the Y axis.
    transform: transform ? `translate3d(0, ${Math.round(transform.y)}px, 0)` : undefined,
    // An inline transition outranks any `motion-reduce:` class, so it goes here.
    transition: reducedMotion ? undefined : transition,
  }
  return (
    <li
      ref={setNodeRef}
      style={style}
      className={cn(
        "group/pr-row flex items-center gap-2 rounded-lg px-1 py-1.5 hover:bg-muted/50",
        isDragging && "z-10 bg-muted shadow-lg",
      )}
    >
      <button
        type="button"
        ref={setActivatorNodeRef}
        {...attributes}
        {...listeners}
        aria-label={`Drag ${prName(pr)}`}
        className={cn(
          "flex shrink-0 cursor-grab touch-manipulation items-center justify-center rounded-md text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
          ROW_CONTROL,
        )}
      >
        <GripVertical className="size-4" />
      </button>
      <PrRowText pr={pr} />
      <RowMenu pr={pr} prKey={prKey} sessionId={sessionId} first={first} last={last} />
    </li>
  )
}

function RowMenu({
  pr,
  prKey,
  sessionId,
  first,
  last,
}: {
  pr: PrView
  prKey: PrKeyView
  sessionId: string
  first: boolean
  last: boolean
}) {
  const action = prRowAction(pr)
  return (
    <DropdownMenu>
      {/* Reveals on hover or focus, stays while its menu is open, and is
        * always there under a finger, which has no hover. */}
      <div
        className={cn(
          "flex shrink-0 items-center transition-opacity duration-200 motion-reduce:transition-none md:opacity-0 md:group-hover/pr-row:opacity-100 md:group-focus-within/pr-row:opacity-100 md:has-[[data-popup-open]]:opacity-100",
          ALWAYS_REVEALED_ON_TOUCH,
        )}
      >
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Actions for ${prName(pr)}`}
              className={ROW_CONTROL}
            />
          }
        >
          <Ellipsis />
        </DropdownMenuTrigger>
      </div>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          render={<a href={pr.url} target="_blank" rel="noopener noreferrer" />}
        >
          <ExternalLink />
          Open on GitHub
        </DropdownMenuItem>
        {first ? null : (
          <>
            <DropdownMenuItem onClick={() => movePullRequest(sessionId, prKey, "top")}>
              <ArrowUpToLine />
              Move to top
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => movePullRequest(sessionId, prKey, "up")}>
              <ArrowUp />
              Move up
            </DropdownMenuItem>
          </>
        )}
        {last ? null : (
          <DropdownMenuItem onClick={() => movePullRequest(sessionId, prKey, "down")}>
            <ArrowDown />
            Move down
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        {/* Neither confirms: Hide comes back from the Hidden section and a
          * detached pull request is attached again from this dialog. */}
        {action === "detach" ? (
          <DropdownMenuItem onClick={() => detachPullRequest(sessionId, prKey)}>
            <Unlink />
            Detach
          </DropdownMenuItem>
        ) : (
          <DropdownMenuItem onClick={() => hidePullRequest(sessionId, prKey)}>
            <EyeOff />
            Hide
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function HiddenSection({ sessionId, hidden }: { sessionId: string; hidden: PrView[] }) {
  return (
    <Collapsible className="mt-1">
      <CollapsibleTrigger
        render={
          <button
            type="button"
            className="group/hidden flex h-8 items-center gap-1.5 rounded-md px-2 text-sm text-muted-foreground hover:text-foreground pointer-coarse:h-10"
          />
        }
      >
        <ChevronRight className="size-4 transition-transform group-data-[panel-open]/hidden:rotate-90 motion-reduce:transition-none" />
        Hidden
        <span className={QUIET_TAG}>{hidden.length}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="flex flex-col gap-1">
          {hidden.map((pr) => {
            const key = prKeyOf(pr)
            return (
              <li key={pr.url} className="flex items-center gap-2 rounded-lg px-1 py-1.5">
                {/* Stands in for the grip, so hidden rows line up with the
                  * shown ones above. */}
                <span aria-hidden className={cn("shrink-0", ROW_CONTROL)} />
                {/* Show again goes under the text once the two no longer fit
                  * side by side, rather than squeezing the title. */}
                <div className="flex min-w-0 flex-1 flex-wrap items-center justify-end gap-2">
                  <div className="flex min-w-48 flex-1 basis-48 items-center gap-2">
                    <PrRowText pr={pr} muted />
                  </div>
                  {key ? (
                    <Button
                      variant="outline"
                      className="h-8 shrink-0 pointer-coarse:h-10"
                      onClick={() => showPullRequest(sessionId, key)}
                    >
                      <Eye />
                      Show again
                    </Button>
                  ) : null}
                </div>
              </li>
            )
          })}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  )
}

type BranchesState = { data: PrBranchesView | null; error: string | null }

// The branch log, read while the dialog is open and read again whenever the
// agent's pull requests, their order or its branch list move, which is what
// `pr_revision` counts.
function useBranches(sessionId: string, revision: number): BranchesState {
  const [state, setState] = useState<BranchesState>({ data: null, error: null })
  useEffect(() => {
    let live = true
    sessionsApi.pullRequestBranches(sessionId).then(
      (data) => {
        if (live) setState({ data, error: null })
      },
      (e: unknown) => {
        if (live)
          setState((prev) => ({
            data: prev.data,
            error: e instanceof Error ? e.message : "Could not read the branches.",
          }))
      },
    )
    return () => {
      live = false
    }
  }, [sessionId, revision])
  return state
}

function formatFirstSeen(iso: string): string | null {
  const ms = Date.parse(iso)
  if (Number.isNaN(ms)) return null
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  })
}

function BranchesView({
  prs,
  hidden,
  branches,
}: {
  prs: PrView[]
  hidden: PrView[]
  branches: BranchesState
}) {
  const find = (key: PrKeyView) =>
    prs.find((pr) => {
      const own = prKeyOf(pr)
      return own !== null && samePrKey(own, key)
    })
  const isHidden = (key: PrKeyView) =>
    hidden.some((pr) => {
      const own = prKeyOf(pr)
      return own !== null && samePrKey(own, key)
    })
  if (!branches.data) {
    return (
      <p className="py-6 text-center text-sm text-muted-foreground">
        {branches.error ?? "Reading the branches…"}
      </p>
    )
  }
  const { attached_on_no_branch: loose } = branches.data
  // The branch checked out now on top, then the rest in the server's order,
  // newest first.
  const rows = [
    ...branches.data.branches.filter((b) => b.current),
    ...branches.data.branches.filter((b) => !b.current),
  ]
  return (
    <div className="flex max-h-[50dvh] flex-col gap-1 overflow-y-auto">
      {branches.error ? (
        <p className="text-xs text-destructive">{branches.error}</p>
      ) : null}
      <ul className="flex flex-col gap-1">
        {rows.map((branch) => {
          const seen = branch.first_seen_at ? formatFirstSeen(branch.first_seen_at) : null
          return (
            <li key={branch.name} className="flex items-center gap-3 rounded-lg px-1 py-1.5">
              <GitBranch className="size-4 shrink-0 text-muted-foreground" />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="flex min-w-0 items-center gap-2">
                  <span data-slot="branch-name" className="truncate font-medium">
                    {branch.name}
                  </span>
                  {branch.current ? <span className={QUIET_TAG}>Current</span> : null}
                </span>
                <span className="flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
                  {seen ? <span>first seen {seen}</span> : null}
                  {branch.former_names.length > 0 ? (
                    <span>
                      was{" "}
                      {branch.former_names.map((former, i) => (
                        <span key={former}>
                          {i > 0 ? ", " : null}
                          <InlineCode>{former}</InlineCode>
                        </span>
                      ))}
                    </span>
                  ) : null}
                </span>
              </span>
              {/* The chips are 40px square at least under a finger, with
                  gap-2 between neighbours on either axis. */}
              <span className="flex shrink-0 flex-wrap items-center justify-end gap-2 text-sm">
                {branch.prs.length === 0 ? (
                  <span className="text-muted-foreground">No pull request</span>
                ) : (
                  branch.prs.map((key) => {
                    const pr = find(key)
                    return (
                      <span key={keyId(key)} className="inline-flex items-center gap-1">
                        <a
                          href={pr?.url}
                          target="_blank"
                          rel="noopener noreferrer"
                          className={cn(
                            "inline-flex items-center justify-end gap-1 rounded pl-1 font-medium hover:underline pointer-coarse:h-10 pointer-coarse:min-w-10",
                            pr ? prIconClass(pr.state) : "text-muted-foreground",
                          )}
                        >
                          <GitPullRequest className="size-3.5" />
                          {pr ? prName(pr) : `#${key.number}`}
                        </a>
                        {isHidden(key) ? (
                          <span className="text-muted-foreground">hidden</span>
                        ) : null}
                      </span>
                    )
                  })
                )}
              </span>
            </li>
          )
        })}
      </ul>
      {loose.map((key) => {
        const pr = find(key)
        return (
          <p key={keyId(key)} className="mt-1 text-sm text-muted-foreground">
            <InlineCode>{pr ? prName(pr) : `${key.owner_repo}#${key.number}`}</InlineCode>{" "}
            was attached by you, so it belongs to no branch here.
          </p>
        )
      })}
    </div>
  )
}
