#!/usr/bin/env node

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const PLATFORM_TARGETS = [
  { platform: "darwin", arch: "x64", key: "darwin-x64" },
  { platform: "darwin", arch: "arm64", key: "darwin-arm64" },
  { platform: "linux", arch: "x64", key: "linux-x64" },
  { platform: "linux", arch: "arm64", key: "linux-arm64" }
];

const current = PLATFORM_TARGETS.find(
  (target) => target.platform === process.platform && target.arch === process.arch
);

if (!current) {
  throw new Error(
    `Unsupported platform combination: ${process.platform}/${process.arch}. ` +
      "Please open an issue to request support: " +
      "https://github.com/patrickdappollonio/dux/issues/new"
  );
}

const binaryName = "dux";
const vendorRoot = path.join(__dirname, "..", "vendor", current.key);
const binaryPath = path.join(vendorRoot, binaryName);

if (!existsSync(binaryPath)) {
  throw new Error(
    `dux binary not found for ${current.key}. ` +
      "Please reinstall the package or open an issue: " +
      "https://github.com/patrickdappollonio/dux/issues/new"
  );
}

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  env: process.env
});

child.on("error", (err) => {
  console.error(`Failed to start dux: ${err.message}`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
