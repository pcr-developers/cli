// Auto-generated-shape platform selector for pcr-dev.
// Resolves the .node addon from the matching @pcr-dev/<triple> optional
// subpackage at runtime. Because the .node file is loaded into the
// already-trusted `node.exe`, AppLocker never sees a separate CreateProcess —
// which is the whole point of shipping this way.

"use strict";

const { platform, arch } = process;

function load(triple) {
  return require(`pcr-dev-${triple}`);
}

let addon;

switch (platform) {
  case "darwin":
    if (arch === "x64") addon = load("darwin-x64");
    else if (arch === "arm64") addon = load("darwin-arm64");
    break;
  case "linux":
    if (arch === "x64") addon = load("linux-x64-gnu");
    break;
  case "win32":
    if (arch === "x64") addon = load("win32-x64-msvc");
    break;
}

if (!addon) {
  throw new Error(
    `Unsupported platform/arch: ${platform}-${arch}. Install from https://github.com/pcr-developers/cli/releases`
  );
}

module.exports = addon;
