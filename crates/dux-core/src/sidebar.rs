//! Core-owned projection of how projects and sessions group into the sidebar
//! tree. Both the TUI and the web render from this single grouping so neither
//! surface re-derives ordering, partitioning, or orphan handling at the
//! interface — the surfaces apply only display state (collapse, selection) on
//! top of it. Kept dependency-light and pure so it is trivially unit-testable.

use serde::Serialize;

use crate::model::{AgentSession, Project};

/// A project's sessions, grouped for the sidebar. `orphaned` marks a group whose
/// project record no longer exists — its sessions outlived a removed project; its
/// `name` is then a short id slice and it always has agents.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SidebarGroup {
    pub project_id: String,
    pub name: String,
    pub orphaned: bool,
    pub path_missing: bool,
    /// Session ids in this group, in engine order.
    pub session_ids: Vec<String>,
}

/// The ordered sidebar grouping. `groups` lists projects-with-agents first
/// (orphan groups appended), then — when split — the projects with no agents.
/// `agentless_start`, when `Some(i)`, is the index in `groups` where the
/// "projects with no agents" section begins; the surfaces draw a separator
/// before it. `None` means no split (below the threshold, or nothing to sink).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SidebarModel {
    pub groups: Vec<SidebarGroup>,
    pub agentless_start: Option<usize>,
}

/// The short display name used for an orphaned (project-less) group — the same
/// 8-char id slice both surfaces show.
pub fn short_project_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Build the sidebar grouping. Mirrors the historical TUI ordering: projects in
/// their stored order, and — only when `empty_separator_min_projects > 0` and
/// the project count meets it — the projects with no agents are sunk below a
/// separator. Sessions whose project no longer exists are surfaced as `orphaned`
/// groups appended after the agent-bearing projects, so neither surface silently
/// drops them.
pub fn build_sidebar(
    projects: &[Project],
    sessions: &[AgentSession],
    empty_separator_min_projects: u16,
) -> SidebarModel {
    use std::collections::{HashMap, HashSet};

    let split = empty_separator_min_projects > 0
        && projects.len() >= usize::from(empty_separator_min_projects);

    // Bucket sessions by project id in one O(sessions) pass, preserving engine
    // order within each project, so the per-project lookups below are O(1) and
    // the whole function is O(projects + sessions).
    let mut by_project: HashMap<&str, Vec<String>> = HashMap::new();
    for session in sessions {
        by_project
            .entry(session.project_id.as_str())
            .or_default()
            .push(session.id.clone());
    }
    let known: HashSet<&str> = projects.iter().map(|p| p.id.as_str()).collect();

    let mut groups: Vec<SidebarGroup> = Vec::new();
    let mut agentless: Vec<SidebarGroup> = Vec::new();
    for project in projects {
        let session_ids = by_project
            .get(project.id.as_str())
            .cloned()
            .unwrap_or_default();
        let group = SidebarGroup {
            project_id: project.id.clone(),
            name: project.name.clone(),
            orphaned: false,
            path_missing: project.path_missing,
            session_ids,
        };
        if split && group.session_ids.is_empty() {
            agentless.push(group);
        } else {
            groups.push(group);
        }
    }

    // Orphan groups: distinct session.project_id values with no project record,
    // in first-seen order, appended after the agent-bearing projects.
    let mut seen: HashSet<&str> = HashSet::new();
    for session in sessions {
        let id = session.project_id.as_str();
        if !known.contains(id) && seen.insert(id) {
            groups.push(SidebarGroup {
                project_id: session.project_id.clone(),
                name: short_project_id(id),
                orphaned: true,
                path_missing: false,
                session_ids: by_project.get(id).cloned().unwrap_or_default(),
            });
        }
    }

    // Only sink the agent-less projects below a separator when there is both
    // something above it and something to sink — matching the TUI's guard.
    let agentless_start = if !agentless.is_empty() && !groups.is_empty() {
        let start = groups.len();
        groups.extend(agentless);
        Some(start)
    } else {
        groups.extend(agentless);
        None
    };

    SidebarModel {
        groups,
        agentless_start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session};

    #[test]
    fn groups_projects_in_order_without_split_below_threshold() {
        let projects = vec![sample_project("p1", "/a"), sample_project("p2", "/b")];
        let sessions = vec![sample_session("s1", "p1", "feat")];
        // Threshold disabled (0): no split, projects stay in stored order.
        let model = build_sidebar(&projects, &sessions, 0);
        assert_eq!(model.agentless_start, None);
        let ids: Vec<&str> = model.groups.iter().map(|g| g.project_id.as_str()).collect();
        assert_eq!(ids, vec!["p1", "p2"]);
        assert_eq!(model.groups[0].session_ids, vec!["s1".to_string()]);
        assert!(model.groups[1].session_ids.is_empty());
        assert!(!model.groups[0].orphaned);
    }

    #[test]
    fn sinks_agentless_projects_below_separator_at_threshold() {
        let projects = vec![sample_project("p1", "/a"), sample_project("p2", "/b")];
        let sessions = vec![sample_session("s1", "p1", "feat")];
        // Threshold 2, two projects, one agent-less -> split.
        let model = build_sidebar(&projects, &sessions, 2);
        assert_eq!(model.agentless_start, Some(1));
        let ids: Vec<&str> = model.groups.iter().map(|g| g.project_id.as_str()).collect();
        assert_eq!(ids, vec!["p1", "p2"]);
    }

    #[test]
    fn no_separator_when_nothing_is_agentless() {
        let projects = vec![sample_project("p1", "/a"), sample_project("p2", "/b")];
        let sessions = vec![
            sample_session("s1", "p1", "feat"),
            sample_session("s2", "p2", "feat"),
        ];
        let model = build_sidebar(&projects, &sessions, 2);
        assert_eq!(model.agentless_start, None);
    }

    #[test]
    fn surfaces_orphaned_sessions_as_a_short_id_group() {
        let projects = vec![sample_project("p1", "/a")];
        let sessions = vec![
            sample_session("s1", "p1", "feat"),
            sample_session("s2", "3fc34174-4561-4ac6-98fb-5f1434c101c2", "feat"),
            sample_session("s3", "3fc34174-4561-4ac6-98fb-5f1434c101c2", "feat"),
        ];
        let model = build_sidebar(&projects, &sessions, 0);
        assert_eq!(model.groups.len(), 2);
        let orphan = &model.groups[1];
        assert!(orphan.orphaned);
        assert_eq!(orphan.name, "3fc34174");
        assert_eq!(orphan.session_ids, vec!["s2".to_string(), "s3".to_string()]);
    }

    #[test]
    fn orphans_precede_the_agentless_separator() {
        let projects = vec![sample_project("p1", "/a"), sample_project("p2", "/b")];
        let sessions = vec![
            sample_session("s1", "p1", "feat"),
            sample_session("s2", "ghost-id", "feat"),
        ];
        // Split (threshold 2): p1 has agents, p2 has none, plus an orphan group.
        let model = build_sidebar(&projects, &sessions, 2);
        let ids: Vec<&str> = model.groups.iter().map(|g| g.project_id.as_str()).collect();
        // Agent-bearing project, then orphan, then the separator, then p2.
        assert_eq!(ids, vec!["p1", "ghost-id", "p2"]);
        assert_eq!(model.agentless_start, Some(2));
    }
}
