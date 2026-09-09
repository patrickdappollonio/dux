// The launcher's one verb, decided by the workspace's project count and nothing else. Shared,
// because the launcher corner's filled button and the empty list's hero button ask the same
// question and two hand-written checks would drift. `null` means the spine has not arrived and
// reads as "new agent", so a workspace that has projects never flashes "Add project": the flip
// happens only on a confirmed zero, the one state where "New agent" has nothing to pick.

export type LauncherVerb = "new-agent" | "add-project"

export function launcherVerb(projectCount: number | null): LauncherVerb {
  return projectCount === 0 ? "add-project" : "new-agent"
}
