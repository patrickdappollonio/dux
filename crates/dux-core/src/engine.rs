//! The headless `Engine`: the single owner of dux's domain state. Surfaces (the
//! TUI `App` today, the web server later) embed/drive it. In E2 it is a passive
//! state container; domain operations and workers move into `Engine` methods in E3.

use crate::config::{Config, DuxPaths};
use crate::lockfile::SingleInstanceLock;
use crate::model::{AgentSession, ChangedFile, Project};
use crate::storage::SessionStore;

pub struct Engine {
    pub config: Config,
    pub paths: DuxPaths,
    pub session_store: SessionStore,
    pub projects: Vec<Project>,
    pub sessions: Vec<AgentSession>,
    pub staged_files: Vec<ChangedFile>,
    pub unstaged_files: Vec<ChangedFile>,
    pub terminal_counter: usize,
    pub github_integration_enabled: bool,
    pub single_instance_lock: SingleInstanceLock,
}
