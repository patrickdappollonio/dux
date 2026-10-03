// `dux server` on a tailnet where `tailscale serve` already points at it: the
// banner lists the MagicDNS and HTTPS URLs beside the listeners, and the two QR
// codes sit side by side under it, the Tailscale IP on the left and the HTTPS
// name on the right.
//
// There is no tailnet in the capture container, so the Tailscale CLI is a
// stand-in answering with an obviously fake machine and tailnet, in the shapes
// the real CLI prints. Nothing here is a real name or address.
module.exports = async ({ sleep, waitFor }) => {
  await waitFor("Scan to open dux", 30000)
  await sleep(600)
}

module.exports.launch = "server"

module.exports.tailscale = {
  ip: "100.101.102.103",
  status: JSON.stringify({
    BackendState: "Running",
    TailscaleIPs: ["100.101.102.103"],
    Self: {
      HostName: "demo-box",
      DNSName: "demo-box.example-tailnet.ts.net.",
      TailscaleIPs: ["100.101.102.103"],
      Online: true,
    },
    MagicDNSSuffix: "example-tailnet.ts.net",
    CurrentTailnet: {
      Name: "example-tailnet",
      MagicDNSSuffix: "example-tailnet.ts.net",
      MagicDNSEnabled: true,
    },
    CertDomains: ["demo-box.example-tailnet.ts.net"],
  }),
  serve: JSON.stringify({
    TCP: { 443: { HTTPS: true } },
    Web: {
      "demo-box.example-tailnet.ts.net:443": {
        Handlers: { "/": { Proxy: "http://127.0.0.1:3890" } },
      },
    },
  }),
}

// The default port, said rather than assumed, because both URLs carry it.
module.exports.config = (text) => text.replace(/^port = \d+$/m, "port = 3890")

// Both codes' URLs, the listener row they come from, and the caption over them.
module.exports.expectText = [
  "Scan to open dux",
  "http://100.101.102.103:3890",
  "https://demo-box.example-tailnet.ts.net",
  "Tailscale (HTTPS, tailscale serve)",
]

module.exports.file = "server-qr-codes.png"
module.exports.cols = 160
module.exports.rows = 45
module.exports.theme = "dux_dark"
