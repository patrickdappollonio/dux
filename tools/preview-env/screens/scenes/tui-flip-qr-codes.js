// The start-web-server flip's status screen on a tailnet where `tailscale serve`
// already points at dux: the header lists the MagicDNS and HTTPS URLs, and the
// log shows the same rows and the same two QR codes `dux server` prints. Tall
// enough that the whole block fits the log without scrolling.
//
// There is no tailnet in the capture container, so the Tailscale CLI is a
// stand-in answering with an obviously fake machine and tailnet, in the shapes
// the real CLI prints. Nothing here is a real name or address.
module.exports = async ({ palette, sleep, waitFor }) => {
  await palette("start-web-server")
  await waitFor("dux server running", 30000)
  // The flip reads the machine's name off its own thread once it is serving,
  // so the codes arrive a moment after the screen does.
  await waitFor("https://demo-box.example-tailnet.ts.net", 30000)
  await sleep(800)
}

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

// The port the URLs carry. Config, because the flip serves on the configured
// port and nothing a journey types changes it.
module.exports.config = (text) => text.replace(/^port = \d+$/m, "port = 3890")

// The screen's heading and both codes' URLs.
module.exports.expectText = [
  "dux server running",
  "http://100.101.102.103:3890",
  "https://demo-box.example-tailnet.ts.net",
]

module.exports.file = "tui-flip-qr-codes.png"
module.exports.cols = 160
module.exports.rows = 56
module.exports.theme = "dux_dark"
