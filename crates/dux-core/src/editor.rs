use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKind {
    Cursor,
    VsCode,
    Zed,
    VsCodium,
    SublimeText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedEditor {
    pub kind: EditorKind,
    pub label: &'static str,
    pub config_key: &'static str,
    pub command: String,
}

struct EditorSpec {
    kind: EditorKind,
    label: &'static str,
    config_key: &'static str,
    commands: &'static [&'static str],
    aliases: &'static [&'static str],
}

const EDITOR_SPECS: &[EditorSpec] = &[
    EditorSpec {
        kind: EditorKind::Cursor,
        label: "Cursor",
        config_key: "cursor",
        commands: &["cursor"],
        aliases: &["cursor"],
    },
    EditorSpec {
        kind: EditorKind::VsCode,
        label: "VS Code",
        config_key: "vscode",
        commands: &["code", "code-insiders"],
        aliases: &["vscode", "code", "code-insiders"],
    },
    EditorSpec {
        kind: EditorKind::Zed,
        label: "Zed",
        config_key: "zed",
        commands: &["zed"],
        aliases: &["zed"],
    },
    EditorSpec {
        kind: EditorKind::VsCodium,
        label: "VSCodium",
        config_key: "vscodium",
        commands: &["codium", "vscodium"],
        aliases: &["vscodium", "codium"],
    },
    EditorSpec {
        kind: EditorKind::SublimeText,
        label: "Sublime Text",
        config_key: "sublime",
        commands: &["subl"],
        aliases: &["sublime", "sublimetext", "subl"],
    },
];

pub fn detect_installed_editors() -> Vec<DetectedEditor> {
    let mut executable_names = Vec::new();
    let mut seen_dirs = HashSet::new();
    let path = env::var_os("PATH").unwrap_or_default();

    for dir in env::split_paths(&path) {
        if !seen_dirs.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                executable_names.push(name.to_string());
            }
        }
    }

    detect_editors_from_names(executable_names.iter().map(String::as_str))
}

pub fn preferred_editor(detected: &[DetectedEditor], configured: &str) -> Option<DetectedEditor> {
    let configured = normalize_editor_name(configured);
    if !configured.is_empty()
        && let Some(editor) = detected
            .iter()
            .find(|editor| matches_configured_editor(editor, &configured))
    {
        return Some(editor.clone());
    }
    detected.first().cloned()
}

pub fn matches_configured_editor(editor: &DetectedEditor, configured: &str) -> bool {
    let configured = normalize_editor_name(configured);
    if configured.is_empty() {
        return false;
    }
    spec_matches(spec_for_kind(editor.kind), &configured)
}

/// The static label for a known editor name/alias/command (e.g. "subl" → "Sublime
/// Text"), whether or not it is installed. Lets a caller name a requested-but-
/// missing editor in an error message. Returns `None` for an unrecognized value.
pub fn editor_label(name: &str) -> Option<&'static str> {
    let normalized = normalize_editor_name(name);
    if normalized.is_empty() {
        return None;
    }
    EDITOR_SPECS
        .iter()
        .find(|spec| spec_matches(spec, &normalized))
        .map(|spec| spec.label)
}

/// Whether a normalized editor name matches a spec by its config key, an alias,
/// or one of its commands. `configured` must already be normalized.
fn spec_matches(spec: &EditorSpec, configured: &str) -> bool {
    configured == spec.config_key
        || spec.aliases.contains(&configured)
        || spec.commands.contains(&configured)
}

pub fn launch_editor(editor: &DetectedEditor, path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("Path does not exist: {}", path.display()));
    }

    Command::new(&editor.command)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch {} via {}", editor.label, editor.command))?;

    Ok(())
}

fn detect_editors_from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<DetectedEditor> {
    let normalized_names = names
        .into_iter()
        .map(normalize_executable_name)
        .collect::<HashSet<_>>();

    EDITOR_SPECS
        .iter()
        .filter_map(|spec| {
            spec.commands.iter().find_map(|command| {
                normalized_names.contains(*command).then(|| DetectedEditor {
                    kind: spec.kind,
                    label: spec.label,
                    config_key: spec.config_key,
                    command: (*command).to_string(),
                })
            })
        })
        .collect()
}

fn normalize_editor_name(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

fn normalize_executable_name(value: &str) -> String {
    Path::new(value)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(value)
        .to_ascii_lowercase()
}

fn spec_for_kind(kind: EditorKind) -> &'static EditorSpec {
    EDITOR_SPECS
        .iter()
        .find(|spec| spec.kind == kind)
        .expect("editor kind should always have a matching spec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_editors_prefers_popular_order() {
        let detected = detect_editors_from_names(["zed", "cursor", "code"].iter().copied());
        let labels = detected
            .iter()
            .map(|editor| editor.label)
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["Cursor", "VS Code", "Zed"]);
    }

    #[test]
    fn preferred_editor_uses_configured_alias_when_available() {
        let detected = detect_editors_from_names(["code", "cursor"].iter().copied());
        let preferred = preferred_editor(&detected, "vscode").expect("editor should resolve");
        assert_eq!(preferred.label, "VS Code");
        assert_eq!(preferred.command, "code");
    }

    #[test]
    fn preferred_editor_falls_back_to_first_detected() {
        let detected = detect_editors_from_names(["zed"].iter().copied());
        let preferred = preferred_editor(&detected, "cursor").expect("editor should resolve");
        assert_eq!(preferred.label, "Zed");
    }

    #[test]
    fn configured_matching_accepts_command_aliases() {
        let detected = detect_editors_from_names(["code-insiders"].iter().copied());
        let editor = detected.first().expect("editor should be detected");
        assert!(matches_configured_editor(editor, "vscode"));
        assert!(matches_configured_editor(editor, "code-insiders"));
    }

    #[test]
    fn detect_finds_newly_supported_editors() {
        // Each new editor resolves from its CLI command, in EDITOR_SPECS order.
        let detected = detect_editors_from_names(["subl", "codium"].iter().copied());
        let labels = detected.iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels, vec!["VSCodium", "Sublime Text"]);
    }

    #[test]
    fn editor_label_resolves_known_names_and_rejects_others() {
        // config key, alias, and command all resolve to the spec label.
        assert_eq!(editor_label("sublime"), Some("Sublime Text"));
        assert_eq!(editor_label("subl"), Some("Sublime Text"));
        assert_eq!(editor_label("codium"), Some("VSCodium"));
        assert_eq!(editor_label("VSCode"), Some("VS Code"));
        // Unknown / empty are not editors.
        assert_eq!(editor_label("emacs"), None);
        assert_eq!(editor_label(""), None);
        // Removed from the supported set.
        assert_eq!(editor_label("antigravity"), None);
        assert_eq!(editor_label("windsurf"), None);
    }

    /// CROSS-LANGUAGE PIN: the web "Open editor" menu (`OPEN_IN_EDITORS` in
    /// editors.ts) must use exactly the dux-core `EDITOR_SPECS` config keys — no
    /// menu entry with a key the server would reject, and no supported editor
    /// missing from the menu. Without this, a typo'd key ("sublime-text") or a
    /// half-added editor would only surface as a runtime 400 on click. Skips when
    /// the web tree isn't present (a crate build outside the workspace).
    #[test]
    fn editor_keys_match_the_typescript_menu() {
        let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dux-web/web/src/lib/editors.ts");
        let Ok(source) = std::fs::read_to_string(&ts_path) else {
            eprintln!("skipping: {} not present", ts_path.display());
            return;
        };
        // Each menu row is `{ key: "<id>", label: ... }`; the `EditorChoice`
        // interface line `key: string` has no quote after `key: ` so it's skipped.
        let mut ts_keys: Vec<&str> = source
            .split("key: \"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        ts_keys.sort_unstable();
        let mut rust_keys: Vec<&str> = EDITOR_SPECS.iter().map(|s| s.config_key).collect();
        rust_keys.sort_unstable();
        assert_eq!(
            rust_keys, ts_keys,
            "the web editor menu (OPEN_IN_EDITORS in editors.ts) and dux-core \
             EDITOR_SPECS config keys have drifted — every menu entry must use a real \
             config key and every supported editor should appear in the menu."
        );
    }
}
