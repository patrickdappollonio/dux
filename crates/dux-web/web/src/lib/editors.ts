// The editors offered in the web "Open editor…" menu. `key` is the dux-core
// editor config key (crates/dux-core/src/editor.rs): the server launches the
// matching CLI on PATH and returns an "isn't installed" error otherwise. Order
// mirrors dux-core's EDITOR_SPECS. Icons render via EditorIcon, keyed by `key`.
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
