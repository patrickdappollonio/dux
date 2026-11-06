// The folder a clone of an address lands in, so the clone dialog can fill in
// a destination while the user types. Twin of `clone_dir_name` in dux-core,
// pinned by the shared fixture both suites read: for a spelling the clone
// rewrites (owner/repo, host/owner/repo, a page pasted from a GitHub host) it
// is the repository's name, the one the clone uses, and otherwise git's own
// rule for the text as typed.

/** The hosts an address is read as GitHub's on: `github.com` always, plus
 * these. Mirrors `dux_core::gh::GithubHosts`, which the bootstrap document
 * carries as `github_hosts`. */
export type GithubHosts = { kind: "hosts"; hosts: string[] } | { kind: "name_rule" }

export const GITHUB_COM_ONLY: GithubHosts = { kind: "hosts", hosts: [] }

/** A `github_hosts` value as it arrived on the wire, made one this build
 * knows: a kind it has never heard of, or a malformed one, is github.com
 * alone, the narrowest reading. */
export function normalizeGithubHosts(value: unknown): GithubHosts {
  if (typeof value === "object" && value !== null && "kind" in value) {
    if (value.kind === "name_rule") return { kind: "name_rule" }
    if (
      value.kind === "hosts" &&
      "hosts" in value &&
      Array.isArray(value.hosts) &&
      value.hosts.every((host: unknown) => typeof host === "string")
    ) {
      return { kind: "hosts", hosts: value.hosts as string[] }
    }
  }
  return GITHUB_COM_ONLY
}

function isGithubHost(hosts: GithubHosts, host: string): boolean {
  if (host === "github.com") return true
  return hosts.kind === "name_rule" ? host.startsWith("github.") : hosts.hosts.includes(host)
}

const U64_MAX = 18446744073709551615n

function stripDotGit(value: string): string {
  return value.length > 4 && value.slice(-4).toLowerCase() === ".git" ? value.slice(0, -4) : value
}

const isName = (part: string) => /^[A-Za-z0-9._-]+$/.test(part)

// A web address's host as a browser spells it, or null for an authority this
// grammar does not read. The twin of `typed_web_host`.
function webHost(authority: string, scheme: string): string | null {
  const colon = authority.indexOf(":")
  const name = colon === -1 ? authority : authority.slice(0, colon)
  const port = colon === -1 ? "" : authority.slice(colon + 1)
  const labels = name.split(".")
  const last = (labels[labels.length - 1] ?? "").toLowerCase()
  if (
    !/^[A-Za-z0-9.-]*$/.test(name) ||
    labels.some((label) => label === "") ||
    /^[0-9]*$/.test(last) ||
    last.startsWith("0x")
  ) {
    return null
  }
  const lowered = name.toLowerCase()
  if (port === "") return lowered
  if (!/^[0-9]+$/.test(port)) return null
  const number = BigInt(port)
  if (number > 65535n) return null
  const own = scheme === "https" ? 443n : 80n
  return number === own ? lowered : `${lowered}:${number}`
}

// The repository's name when the clone rewrites `text`, null when the clone
// hands it to git as typed. The twin of `typed_shape` in dux-core, rule for
// rule; dux-core also asks the pull-request field's parser, and a test there
// holds that the parser never reads one of these differently.
function typedRepositoryName(text: string, hosts: GithubHosts): string | null {
  if (text === "" || !/^[\x21-\x7e]+$/.test(text) || text.includes("\\") || /^[/.~]/.test(text)) {
    return null
  }
  const lowered = text.toLowerCase()
  const scheme = ["https", "http"].find((candidate) => lowered.startsWith(`${candidate}://`))
  const rest = text.slice(scheme === undefined ? 0 : scheme.length + "://".length)
  // The page is everything before the first `?` or `#`; a fragment is what
  // follows the first `#`.
  const cut = rest.search(/[?#]/)
  const page = cut === -1 ? rest : rest.slice(0, cut)
  const tail = cut === -1 ? "" : rest.slice(cut)
  const hash = rest.indexOf("#")
  const fragment = hash === -1 ? null : rest.slice(hash + 1)
  const segments = page.split("/").filter((part) => part !== "")
  if (segments.some((part) => /^\.+$/.test(part))) return null
  let path: string[]
  if (scheme !== undefined) {
    if (page.includes("%") || (fragment?.includes("%") ?? false)) return null
    const host = webHost(page.split("/")[0] ?? "", scheme)
    if (host === null) return null
    // Only a page on a GitHub host is rewritten; anything else, and an
    // address that is exactly the repository, goes as typed.
    if (!isGithubHost(hosts, host) || (segments.length <= 3 && tail === "")) return null
    path = segments.slice(1)
  } else {
    const first = segments[0]
    if (first === undefined) return null
    const looksLikeHost = /[.:]/.test(first) || first.toLowerCase() === "localhost"
    const host = looksLikeHost ? first.toLowerCase() : "github.com"
    path = looksLikeHost ? segments.slice(1) : segments
    const github = !looksLikeHost || isGithubHost(hosts, host)
    // Only a GitHub host has a page to cut a query or a fragment off;
    // elsewhere the text goes as typed. A colon is a port or scp, and an `@` a
    // user: neither has a short form.
    if ((!github && tail !== "") || /[:@]/.test(page)) return null
    if (looksLikeHost && !(host.includes(".") && host.split(".").every((label) => label !== ""))) {
      return null
    }
    if (path.length > 2 && (!looksLikeHost || !github)) return null
  }
  if (path.length < 2) return null
  const owner = path[0]
  const repo = stripDotGit(path[1])
  if (!isName(owner) || !isName(repo) || repo.toLowerCase() === ".git") return null
  if (path.length === 2 && fragment !== null && /^[0-9]+$/.test(fragment) && BigInt(fragment) > U64_MAX) {
    return null
  }
  return repo
}

/**
 * The repository's name for a spelling the clone rewrites, asked with the same
 * hosts the server reads it with; otherwise git's own rule: trim, drop
 * trailing `/`, drop a trailing `.git`, drop trailing `/` again, and take what
 * follows the last `/` or `:` (the `:` is an scp-style address's separator).
 * Null when nothing usable is left.
 */
export function cloneDirName(address: string, hosts: GithubHosts): string | null {
  const trimmed = address.trim()
  const repository = typedRepositoryName(trimmed, hosts)
  if (repository !== null) return repository
  let rest = trimmed.replace(/\/+$/, "")
  if (rest.endsWith(".git")) rest = rest.slice(0, -".git".length)
  rest = rest.replace(/\/+$/, "")
  const name = rest.split(/[/:]/).pop() ?? ""
  return name === "" ? null : name
}
