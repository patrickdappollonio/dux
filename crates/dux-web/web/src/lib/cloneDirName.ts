// The folder `git clone` would pick for an address, so the clone dialog can
// fill in a destination while the user types. Twin of `clone_dir_name` in
// dux-core, pinned by the shared fixture both suites read.

/**
 * Git's own rule: trim, drop trailing `/`, drop a trailing `.git`, drop
 * trailing `/` again, and take what follows the last `/` or `:` (the `:` is an
 * scp-style address's separator). Null when nothing usable is left.
 */
export function cloneDirName(address: string): string | null {
  let rest = address.trim().replace(/\/+$/, "")
  if (rest.endsWith(".git")) rest = rest.slice(0, -".git".length)
  rest = rest.replace(/\/+$/, "")
  const name = rest.split(/[/:]/).pop() ?? ""
  return name === "" ? null : name
}
