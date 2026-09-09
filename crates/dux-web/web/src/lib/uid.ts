// Pure client-id generator. `crypto.randomUUID` exists only on secure contexts, and dux is
// frequently served over plain HTTP on a LAN or Tailscale address, where `crypto` is present
// but `randomUUID` is not; `getRandomValues` works there, and a `Math.random` fallback covers
// a `crypto` that is missing entirely.
export function newClientId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID()
  }
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    const bytes = crypto.getRandomValues(new Uint8Array(16))
    // Stamp the RFC 4122 version/variant bits so the fallback still looks
    // like a v4 UUID (not load-bearing for uniqueness, just format parity).
    bytes[6] = (bytes[6] & 0x0f) | 0x40
    bytes[8] = (bytes[8] & 0x3f) | 0x80
    const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
  }
  // No Web Crypto at all: a non-cryptographic id is enough for a client-only tab identifier,
  // which is never sent to the server as a security token.
  return `id-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}
