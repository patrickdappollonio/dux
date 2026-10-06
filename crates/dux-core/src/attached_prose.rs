//! Who a destructive change would cut off, in the words both surfaces print.
//!
//! A delete, stop, tab or terminal close, project removal or quit that somebody
//! else is attached to lists them in its dialog, one line each, under one
//! opening line, before offering to go ahead anyway. The terminal UI renders
//! these, and the browser's `lib/attached.ts` builds the same segments;
//! `tests/fixtures/prose_cross_language.json` pins the two together.

use crate::attachments::{Surface, TargetKind};
use crate::prose::Prose;

/// One attachment in the way, with every name already resolved by the
/// surface showing it: the device's short label, the tab's or terminal's
/// label, and the agent's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedEntry {
    /// The device's short label, or `None` when it could not be read.
    pub device: Option<String>,
    pub surface: Surface,
    pub address: Option<String>,
    pub verified: bool,
    pub driving: bool,
    pub target_kind: TargetKind,
    pub target_label: String,
    /// The agent the tab or terminal belongs to, when it belongs to one.
    pub agent_label: Option<String>,
}

/// The line above the list.
pub fn attached_lead_prose() -> Prose {
    Prose::new().text("Someone else is using this right now. Going ahead cuts them off:")
}

/// One line of the list: who, from where, typing or watching, and in what.
pub fn attached_entry_prose(entry: &AttachedEntry) -> Prose {
    let mut prose = match (&entry.device, entry.surface) {
        (Some(device), _) => Prose::new().name(device.clone()),
        (None, Surface::Browser) => Prose::new().text("a browser"),
        (None, Surface::TerminalUi) => Prose::new().name(crate::background_serve::TUI_DEVICE_LABEL),
    };
    if let Some(address) = &entry.address {
        prose.push_text(format!(" at {address}"));
        if !entry.verified {
            prose.push_text(" (unverified)");
        }
    }
    prose.push_text(if entry.driving {
        ", typing in "
    } else {
        ", watching "
    });
    prose.push_text(match entry.target_kind {
        TargetKind::Tab => "tab ",
        TargetKind::Terminal => "terminal ",
    });
    prose.push_name(entry.target_label.clone());
    if let Some(agent) = &entry.agent_label {
        prose.push_text(" of agent ");
        prose.push_name(agent.clone());
    }
    prose
}
