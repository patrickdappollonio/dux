// Pure client-id generator. `crypto.randomUUID` only exists on secure
// contexts (https, or localhost); dux is frequently run over plain HTTP on a
// LAN/Tailscale address (see `lib/sw.ts`'s `isSecureContext` gating for the
// same deployment reality), where `crypto` is present but `randomUUID` is
// undefined. Falling back to `crypto.getRandomValues` (available even on an
// insecure context) keeps tab-id generation from throwing there; a final
// `Math.random` fallback covers the practically nonexistent, but still
// guarded defensively, case where `crypto` itself is unavailable.
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
  // No Web Crypto at all: fall back to a non-cryptographic but still unique-
  // enough id for a client-only tab identifier (never sent to the server as a
  // security token).
  return `id-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}
