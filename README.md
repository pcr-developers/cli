# pcr-dev

PCR.dev CLI — capture AI coding prompts for peer review.

## Install

```bash
npm install -g pcr-dev
```

Or via Homebrew (coming soon):
```bash
brew tap pcr-dev/tap
brew install pcr
```

## Quick start

```bash
pcr login              # authenticate with your PCR.dev account
cd your-project
pcr init               # register this project for prompt capture
pcr start              # start the watcher
```

## Commands

| Command | Description |
|---------|-------------|
| `pcr init` | Register the current directory for prompt capture |
| `pcr login` | Authenticate with PCR.dev |
| `pcr logout` | Remove saved credentials |
| `pcr start` | Start the file watcher |
| `pcr mcp` | Start the MCP server on stdio |
| `pcr status` | Show auth and registered project info |

## MCP integration

Add to your Cursor or Claude Code MCP config:

```json
{
  "mcpServers": {
    "pcr": {
      "command": "pcr",
      "args": ["mcp"]
    }
  }
}
```

## How it works

The watcher passively reads Cursor and Claude Code session files from:
- `~/.cursor/projects/` (Cursor agent transcripts)
- `~/.claude/projects/` (Claude Code sessions)

It only captures prompts from projects you've explicitly registered with `pcr init`. All data is sent to your PCR.dev dashboard for review.

## Requirements

- Node.js 20+
- A [PCR.dev](https://pcr.dev) account
