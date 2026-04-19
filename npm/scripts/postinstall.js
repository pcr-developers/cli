#!/usr/bin/env node
/**
 * postinstall.js — downloads the correct pre-built binary from GitHub Releases
 * and places it at npm/lib/pcr-bin.
 *
 * Runs automatically after `npm install -g pcr-dev`.
 * Non-fatal on unsupported platforms — warns instead of failing the install.
 */

"use strict";

const https = require("https");
const fs = require("fs");
const path = require("path");
const os = require("os");

const pkg = require("../package.json");
const VERSION = pkg.version;
const REPO = "pcr-developers/cli";

// Map { platform-arch } to GitHub release asset names
const PLATFORM_MAP = {
  "darwin-arm64": "pcr-macos-arm64",
  "darwin-x64": "pcr-macos-x64",
  "linux-x64": "pcr-linux-x64",
  "win32-x64": "pcr-windows-x64.exe",
};

const platform = os.platform(); // "darwin" | "linux" | "win32"
const arch = os.arch();         // "arm64" | "x64"
const key = `${platform}-${arch}`;
const binaryName = PLATFORM_MAP[key];
const isWindows = platform === "win32";

if (!binaryName) {
  console.warn(
    `PCR: Unsupported platform ${key}. ` +
    `Download a binary manually from https://github.com/${REPO}/releases/tag/v${VERSION}`
  );
  process.exit(0);
}

const downloadURL =
  `https://github.com/${REPO}/releases/download/v${VERSION}/${binaryName}`;

const libDir = path.join(__dirname, "..", "lib");
const destPath = path.join(libDir, isWindows ? "pcr-bin.exe" : "pcr-bin");

if (!fs.existsSync(libDir)) {
  fs.mkdirSync(libDir, { recursive: true });
}

console.log(`PCR: Downloading ${binaryName} v${VERSION}...`);

function download(url, dest, redirects) {
  if (redirects > 5) {
    console.error("PCR: Too many redirects.");
    process.exit(1);
  }
  https.get(url, (res) => {
    if (res.statusCode === 301 || res.statusCode === 302) {
      download(res.headers.location, dest, redirects + 1);
      return;
    }
    if (res.statusCode !== 200) {
      console.error(
        `PCR: Failed to download binary (HTTP ${res.statusCode}).\n` +
        `  URL: ${url}\n` +
        `  Download manually from https://github.com/${REPO}/releases/tag/v${VERSION}`
      );
      process.exit(0); // non-fatal
      return;
    }
    const tmp = dest + ".tmp";
    const out = fs.createWriteStream(tmp);
    res.pipe(out);
    out.on("finish", () => {
      out.close(() => {
        fs.renameSync(tmp, dest);
        fs.chmodSync(dest, 0o755);
        console.log(`PCR: Installed to ${dest}`);
      });
    });
    out.on("error", (err) => {
      fs.unlink(tmp, () => {});
      console.error(`PCR: Write error: ${err.message}`);
      process.exit(0);
    });
  }).on("error", (err) => {
    console.error(`PCR: Download error: ${err.message}`);
    process.exit(0);
  });
}

download(downloadURL, destPath, 0);
