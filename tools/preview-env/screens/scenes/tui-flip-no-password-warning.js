// The start-web-server flip's log viewer with no password set: the flip binds
// this machine and its Tailscale address, the second of which reaches beyond
// this machine, so the log carries the same red warning `dux server` prints.
//
// There is no tailnet in the capture container, so the Tailscale CLI is a
// stand-in answering with an obviously fake machine and tailnet, in the shapes
// the real CLI prints. Nothing here is a real name or address.
module.exports = async ({ palette, sendKeys, sleep, waitFor }) => {
  await palette("start-web-server")
  await waitFor("dux server running", 30000)
  // The log follows its latest line, and the QR codes printed after the
  // warning push it off the top, so the viewer is moved back to its oldest
  // line, where the warning sits under the listeners it is about.
  await sleep(3000)
  sendKeys("Home")
  await waitFor("No password is set", 30000)
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
  serve: JSON.stringify({}),
}

// The port the URLs carry.
module.exports.config = (text) => text.replace(/^port = \d+$/m, "port = 3890")

// The flip's heading and the warning in its log.
module.exports.expectText = [
  "dux server running",
  "No password is set and dux is reachable beyond this machine",
]

module.exports.file = "tui-flip-no-password-warning.png"
module.exports.cols = 160
module.exports.rows = 56
module.exports.theme = "dux_dark"
