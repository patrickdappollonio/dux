import { matchWorkspace, type AgentWorkspaceWire } from "@/lib/agentWorkspace"

// The heading over each group of the pane menu, in one place so no group can be
// headed differently by two anchors. Every group in the menu carries one.
export const MENU_GROUP_LABELS = {
  input: "Input",
  pane: "Pane",
  running: "Running",
  agent: "Agent",
  startup: "Startup",
  remove: "Remove",
  terminal: "Terminal",
  theater: "Theater",
  app: "App",
} as const

/// The group of tab and project entries. An agent with no project has only its
/// tabs, and its heading says so.
export function tabsGroupLabel(workspace: AgentWorkspaceWire): string {
  return matchWorkspace(workspace, {
    managed: () => "Tabs and project",
    folder: () => "Tabs",
  })
}

/// The group of editor, new-terminal and copy-path entries: a managed agent
/// lives in a worktree, a standalone one in the user's own folder, as
/// `newTerminalLabel` words it.
export function locationGroupLabel(workspace: AgentWorkspaceWire): string {
  return matchWorkspace(workspace, {
    managed: () => "Worktree",
    folder: () => "Folder",
  })
}
