import type { StartupLogsScope } from "@/lib/store"

// One dialog serves both scopes of `StartupCommandLogScope`, and only the
// naming differs: the title says which entity and the subtitle says how wide
// the list is, or an agent's runs and a project's would look identical.
export function startupLogsCopy(
  scope: StartupLogsScope,
  projectName: string | undefined,
  agentName: string | undefined,
): { title: string; description: string; emptyMessage: string } {
  if (scope === "project") {
    return {
      title: `Startup command logs: ${projectName || "project"} (all agents)`,
      description:
        "Output from each run of the project startup command across every agent in this project, newest first.",
      emptyMessage:
        "No startup command logs yet. Run the startup command for an agent in this project to generate one.",
    }
  }
  return {
    title: `Startup command logs: ${agentName ?? "agent"}`,
    description:
      "Output from each run of the project startup command in this agent's worktree, newest first.",
    emptyMessage:
      "No startup command logs yet. Run the startup command for this agent to generate one.",
  }
}
