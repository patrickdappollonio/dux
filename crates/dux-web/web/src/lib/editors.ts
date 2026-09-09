// The editors offered in the web "Open editor…" menu. `key` is the dux-core
// editor config key (crates/dux-core/src/editor.rs), which the server resolves
// and `EditorIcon` renders; order mirrors that file's EDITOR_SPECS.
export interface EditorChoice {
  key: string
  label: string
}

export const OPEN_IN_EDITORS: EditorChoice[] = [
  { key: "cursor", label: "Cursor" },
  { key: "vscode", label: "VS Code" },
  { key: "zed", label: "Zed" },
  { key: "vscodium", label: "VSCodium" },
  { key: "sublime", label: "Sublime Text" },
]
