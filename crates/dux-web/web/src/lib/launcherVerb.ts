// The empty list's hero verb, decided by the workspace's project count and nothing else. `null`
// means the spine has not arrived and reads as "new agent", so a workspace that has projects never
// flashes "Add project": the flip happens only on a confirmed zero, the one state where "New agent"
// has nothing to pick.

export type LauncherVerb = "new-agent" | "add-project"

export function launcherVerb(projectCount: number | null): LauncherVerb {
  return projectCount === 0 ? "add-project" : "new-agent"
}
