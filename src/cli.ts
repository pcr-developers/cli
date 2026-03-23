#!/usr/bin/env node

/**
 * PCR.dev CLI
 *
 * Usage:
 *   pcr init      — Register the current directory as a tracked project
 *   pcr login     — Authenticate with PCR.dev
 *   pcr logout    — Remove saved credentials
 *   pcr start     — Start the file watcher (captures prompts from registered projects)
 *   pcr mcp       — Start the MCP server on stdio (for Cursor / Claude Code integration)
 *   pcr status    — Show auth and registered project info
 *   pcr help      — Show this message
 */

import { createRequire } from "module";

const require = createRequire(import.meta.url);
const pkg = require("../package.json") as { version: string };

const command = process.argv[2];

async function main() {
  switch (command) {
    case "init": {
      const { runInit } = await import("./commands/init.js");
      await runInit();
      break;
    }
    case "login": {
      const { runLogin } = await import("./commands/login.js");
      await runLogin();
      break;
    }
    case "logout": {
      const { runLogout } = await import("./commands/logout.js");
      await runLogout();
      break;
    }
    case "start": {
      const { runStart } = await import("./commands/start.js");
      await runStart();
      break;
    }
    case "mcp": {
      const { runMcp } = await import("./commands/mcp.js");
      await runMcp();
      break;
    }
    case "status": {
      const { runStatus } = await import("./commands/status.js");
      await runStatus();
      break;
    }
    case "github": {
      const { runGithub } = await import("./commands/github.js");
      await runGithub(process.argv[3]);
      break;
    }
    case undefined:
    case "help":
    case "--help":
    case "-h": {
      printHelp();
      break;
    }
    case "--version":
    case "-v": {
      console.log(pkg.version);
      break;
    }
    default: {
      console.error(`\nUnknown command: ${command}`);
      printHelp();
      process.exit(1);
    }
  }
}

function printHelp() {
  console.log(`
PCR.dev v${pkg.version} — prompt capture & review

Usage: pcr <command>

Commands:
  init      Register the current directory as a tracked project
  login     Authenticate with PCR.dev
  logout    Remove saved credentials
  start     Start the file watcher
  mcp       Start the MCP server on stdio
  status    Show auth and registered project info
  github    Set up GitHub PR integration
  help      Show this help message

Flags:
  --version, -v    Print version number
  --help, -h       Show this help message

MCP integration (Cursor / Claude Code):
  Add to your MCP config:
  {
    "mcpServers": {
      "pcr": { "command": "pcr", "args": ["mcp"] }
    }
  }
`);
}

main().catch((err: Error) => {
  console.error(`PCR: Error — ${err.message}`);
  process.exit(1);
});
