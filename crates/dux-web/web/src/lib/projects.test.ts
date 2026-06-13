import { describe, expect, it } from "vitest"

import { partitionProjects } from "@/lib/projects"
import type { ProjectView, SessionView, SidebarModel } from "@/lib/types"

// partitionProjects only reads `id` / `project_id`; the rest of the view shapes
// are irrelevant here, so build minimal fixtures.
const project = (id: string) => ({ id }) as unknown as ProjectView
const session = (id: string, project_id: string) =>
  ({ id, project_id }) as unknown as SessionView

describe("partitionProjects", () => {
  it("excludes orphan ids from the reorder payload while still rendering them", () => {
    const sidebar: SidebarModel = {
      groups: [
        {
          project_id: "p1",
          name: "p1",
          orphaned: false,
          path_missing: false,
          session_ids: ["s1"],
        },
        {
          project_id: "p2",
          name: "p2",
          orphaned: false,
          path_missing: false,
          session_ids: [],
        },
        {
          project_id: "ghost",
          name: "ghost",
          orphaned: true,
          path_missing: false,
          session_ids: ["s2"],
        },
      ],
      // p2 onward are the agent-less real projects.
      agentless_start: 1,
    }
    const projects = [project("p1"), project("p2")]
    const sessions = [session("s1", "p1"), session("s2", "ghost")]

    const { withAgents, withoutAgents, realOrder } = partitionProjects(
      sidebar,
      projects,
      sessions,
    )

    // The orphan group is rendered (its sessions must stay reachable)…
    expect(withAgents).toEqual(["p1", "ghost"])
    expect(withoutAgents).toEqual(["p2"])
    // …but the reorder payload the server validates contains ONLY real project
    // ids, in display order (agent-bearing first, then agent-less). A ghost id
    // would be rejected — the server has no project record to reorder.
    expect(realOrder).toEqual(["p1", "p2"])
  })
})
