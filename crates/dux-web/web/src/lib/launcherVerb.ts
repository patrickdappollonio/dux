// The launcher's one verb, decided by the workspace's project count and nothing
// else. Pure and shared, because TWO surfaces ask the same question: the
// launcher corner's filled button (sidebar footer and mobile hub) and the empty
// list's hero button. Two hand-written `projects.length === 0` checks would
// drift on exactly the case below.
//
// `null` means the spine has not arrived yet, and it deliberately reads as
// "new agent": a workspace that already has projects must not flash "Add
// project" for the frame before its spine lands. The flip happens only on a
// CONFIRMED zero, which is the one state where "New agent" would open a picker
// with nothing to pick.

export type LauncherVerb = "new-agent" | "add-project"

export function launcherVerb(projectCount: number | null): LauncherVerb {
  return projectCount === 0 ? "add-project" : "new-agent"
}
