import { describe, expect, it } from "vitest"

import { isLocalAccessHost, isLoopbackName, isPrivateIpv4, parseIpv4 } from "./localAccess"

describe("isLocalAccessHost", () => {
  it("treats localhost and loopback as local", () => {
    expect(isLocalAccessHost("localhost")).toBe(true)
    expect(isLocalAccessHost("LOCALHOST")).toBe(true)
    expect(isLocalAccessHost("foo.localhost")).toBe(true)
    expect(isLocalAccessHost("127.0.0.1")).toBe(true)
    expect(isLocalAccessHost("127.1.2.3")).toBe(true)
    expect(isLocalAccessHost("::1")).toBe(true)
    expect(isLocalAccessHost("[::1]")).toBe(true)
    // 0.0.0.0 (a common same-machine dev URL) counts as local.
    expect(isLocalAccessHost("0.0.0.0")).toBe(true)
  })

  it("treats RFC1918 private IPv4 ranges as local", () => {
    expect(isLocalAccessHost("10.0.0.5")).toBe(true)
    expect(isLocalAccessHost("192.168.1.5")).toBe(true)
    expect(isLocalAccessHost("172.16.0.1")).toBe(true)
    expect(isLocalAccessHost("172.31.255.254")).toBe(true)
  })

  it("treats the 172.16/12 boundaries correctly", () => {
    expect(isLocalAccessHost("172.15.0.1")).toBe(false)
    expect(isLocalAccessHost("172.32.0.1")).toBe(false)
  })

  it("treats Tailscale CGNAT (100.64.0.0/10) as REMOTE", () => {
    expect(isLocalAccessHost("100.64.0.1")).toBe(false)
    expect(isLocalAccessHost("100.100.100.100")).toBe(false)
    expect(isLocalAccessHost("100.127.255.255")).toBe(false)
  })

  it("treats public IPs and domains as remote", () => {
    expect(isLocalAccessHost("8.8.8.8")).toBe(false)
    expect(isLocalAccessHost("1.1.1.1")).toBe(false)
    expect(isLocalAccessHost("getdux.app")).toBe(false)
    expect(isLocalAccessHost("box.tail1234.ts.net")).toBe(false)
    expect(isLocalAccessHost("my-server.local")).toBe(false)
  })

  it("rejects malformed / out-of-range dotted quads", () => {
    expect(isLocalAccessHost("10.0.0.256")).toBe(false)
    expect(isLocalAccessHost("999.1.1.1")).toBe(false)
    expect(isLocalAccessHost("10.0.0")).toBe(false)
  })

  it("treats non-loopback IPv6 (incl. Tailscale ULA) as remote", () => {
    expect(isLocalAccessHost("fd7a:115c:a1e0::1")).toBe(false)
    expect(isLocalAccessHost("fe80::1")).toBe(false)
  })
})

describe("isLoopbackName", () => {
  it("names this machine", () => {
    expect(isLoopbackName("localhost")).toBe(true)
    expect(isLoopbackName("dux.localhost")).toBe(true)
    expect(isLoopbackName("0.0.0.0")).toBe(true)
    expect(isLoopbackName("::1")).toBe(true)
    expect(isLoopbackName("[::1]")).toBe(true)
  })

  it("does not name an address or a domain", () => {
    expect(isLoopbackName("127.0.0.1")).toBe(false)
    expect(isLoopbackName("localhost.example.com")).toBe(false)
    expect(isLoopbackName("fe80::1")).toBe(false)
  })
})

describe("parseIpv4", () => {
  it("reads a dotted quad into its octets", () => {
    expect(parseIpv4("172.16.0.1")).toEqual([172, 16, 0, 1])
  })

  it("refuses an octet above 255", () => {
    expect(parseIpv4("10.0.0.256")).toBeNull()
  })

  it("refuses anything that is not four octets", () => {
    expect(parseIpv4("10.0.0")).toBeNull()
    expect(parseIpv4("getdux.app")).toBeNull()
    expect(parseIpv4("fd7a:115c:a1e0::1")).toBeNull()
  })
})

describe("isPrivateIpv4", () => {
  it("accepts loopback and the RFC1918 ranges", () => {
    expect(isPrivateIpv4([127, 1, 2, 3])).toBe(true)
    expect(isPrivateIpv4([10, 0, 0, 5])).toBe(true)
    expect(isPrivateIpv4([192, 168, 1, 5])).toBe(true)
    expect(isPrivateIpv4([172, 16, 0, 1])).toBe(true)
    expect(isPrivateIpv4([172, 31, 255, 254])).toBe(true)
  })

  it("rejects the 172.16/12 neighbours and the CGNAT range", () => {
    expect(isPrivateIpv4([172, 15, 0, 1])).toBe(false)
    expect(isPrivateIpv4([172, 32, 0, 1])).toBe(false)
    expect(isPrivateIpv4([100, 64, 0, 1])).toBe(false)
    expect(isPrivateIpv4([192, 169, 1, 5])).toBe(false)
    expect(isPrivateIpv4([8, 8, 8, 8])).toBe(false)
  })
})
