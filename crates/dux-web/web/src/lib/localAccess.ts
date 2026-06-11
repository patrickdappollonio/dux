// Whether the dux server is reachable at a "local-access" URL — used to gate the
// editor's "Open in editor" action, which spawns a GUI editor on the SERVER.
//
// "Local" = localhost / loopback (incl. 0.0.0.0) / RFC1918 private IPv4 ranges,
// matching the user-facing rule "localhost or a local IP". Deliberately NOT
// local: Tailscale's CGNAT range (100.64.0.0/10), public IPs, and ANY
// hostname/domain (a letsencrypt domain, a Tailscale MagicDNS name). IPv6 is
// treated as remote except loopback (::1), so a Tailscale IPv6 (fd7a:…) is never
// misread as local.
//
// Caveat: a private LAN IP only means "some host on this network" — it may be a
// DIFFERENT machine than the browser, in which case the editor opens on the
// server rather than the user's screen (a harmless no-op on a headless server).
// Loopback is the only true same-machine guarantee; LAN IPs are included by the
// user's explicit request to enable the feature for local IPs.
export function isLocalAccessHost(hostname: string): boolean {
  const host = hostname.trim().toLowerCase()
  if (
    host === "localhost" ||
    host === "0.0.0.0" ||
    host === "::1" ||
    host === "[::1]" ||
    host.endsWith(".localhost")
  ) {
    return true
  }
  const m = host.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/)
  if (!m) return false
  const oct = [m[1], m[2], m[3], m[4]].map((n) => Number(n))
  if (oct.some((n) => n > 255)) return false
  const [a, b] = oct
  if (a === 127) return true // 127.0.0.0/8 loopback
  if (a === 10) return true // 10.0.0.0/8
  if (a === 192 && b === 168) return true // 192.168.0.0/16
  if (a === 172 && b >= 16 && b <= 31) return true // 172.16.0.0/12
  // 100.64.0.0/10 (Tailscale CGNAT) and everything else fall through as remote.
  return false
}
