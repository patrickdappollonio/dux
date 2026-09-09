// Whether the dux server is reachable at a local-access URL, which gates the editor's "Open in
// editor" action, spawning a GUI editor on the server rather than in the browser. Local means
// localhost, loopback (0.0.0.0 included) and the RFC1918 private IPv4 ranges. Deliberately not
// local: Tailscale's CGNAT range (100.64.0.0/10), public IPs, and any hostname or domain. IPv6
// is remote except ::1, so a Tailscale IPv6 is never misread as local.
//
// A private LAN IP only means "some host on this network", which may be a different machine
// than the browser, where the editor opens on the server rather than the user's screen.
// Loopback is the only true same-machine guarantee; LAN IPs are included by explicit request.
export function isLocalAccessHost(hostname: string): boolean {
  const host = hostname.trim().toLowerCase()
  if (isLoopbackName(host)) return true
  const octets = parseIpv4(host)
  return octets !== null && isPrivateIpv4(octets)
}

// The names that resolve to this machine. `[::1]` appears bracketed because that is the form a
// URL carries; IPv6 otherwise falls through to remote.
export function isLoopbackName(host: string): boolean {
  return (
    host === "localhost" ||
    host === "0.0.0.0" ||
    host === "::1" ||
    host === "[::1]" ||
    host.endsWith(".localhost")
  )
}

// The four octets of a dotted quad, or null for anything else: a domain, an IPv6 literal, a
// short quad, or an octet above 255.
export function parseIpv4(host: string): [number, number, number, number] | null {
  const m = host.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/)
  if (!m) return null
  const oct = [m[1], m[2], m[3], m[4]].map((n) => Number(n))
  if (oct.some((n) => n > 255)) return null
  return oct as [number, number, number, number]
}

// Loopback plus the RFC1918 ranges. 100.64.0.0/10 (Tailscale CGNAT) and everything else fall
// through as remote.
export function isPrivateIpv4([a, b]: readonly number[]): boolean {
  if (a === 127) return true // 127.0.0.0/8 loopback
  if (a === 10) return true // 10.0.0.0/8
  if (a === 192 && b === 168) return true // 192.168.0.0/16
  if (a === 172 && b >= 16 && b <= 31) return true // 172.16.0.0/12
  return false
}
