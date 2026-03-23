/**
 * pcr github — set up GitHub PR integration.
 *
 * Usage:
 *   pcr github setup    — generates webhook secret, deploys Edge Function,
 *                         sets the secret, and prints the webhook URL
 *   pcr github status   — shows whether GitHub is connected and the webhook URL
 */

import { createHash, randomBytes } from "crypto";
import { execSync, spawnSync } from "child_process";
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "fs";
import { join, dirname } from "path";
import { homedir } from "os";
import { PCR_DIR, PCR_APP_URL, PCR_SUPABASE_URL } from "../lib/constants.js";

const GITHUB_CONFIG_FILE = join(homedir(), PCR_DIR, "github.json");

interface GithubConfig {
  webhookSecret: string;
  configuredAt: string;
}

function loadGithubConfig(): GithubConfig | null {
  try {
    if (existsSync(GITHUB_CONFIG_FILE)) {
      return JSON.parse(readFileSync(GITHUB_CONFIG_FILE, "utf-8")) as GithubConfig;
    }
  } catch { /* ignore */ }
  return null;
}

function saveGithubConfig(config: GithubConfig): void {
  const dir = dirname(GITHUB_CONFIG_FILE);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  writeFileSync(GITHUB_CONFIG_FILE, JSON.stringify(config, null, 2));
}

function getWebhookUrl(): string {
  // Derive Edge Function URL from Supabase project URL
  const projectRef = PCR_SUPABASE_URL.replace("https://", "").replace(".supabase.co", "");
  return `https://${projectRef}.supabase.co/functions/v1/github-webhook`;
}

function supabaseAvailable(): boolean {
  const result = spawnSync("supabase", ["--version"], { stdio: "pipe" });
  return result.status === 0;
}

export async function runGithub(subcommand?: string): Promise<void> {
  const cmd = subcommand ?? "status";

  switch (cmd) {
    case "setup":
      await setup();
      break;
    case "status":
      await status();
      break;
    default:
      console.log(`
pcr github — GitHub PR integration

Usage:
  pcr github setup    Set up the webhook (deploys Edge Function, generates secret)
  pcr github status   Show current configuration and webhook URL
`);
  }
}

async function setup(): Promise<void> {
  console.log("\nPCR GitHub integration setup\n");

  // Step 1: generate or reuse webhook secret
  let config = loadGithubConfig();
  let secret: string;

  if (config?.webhookSecret) {
    console.log("  Webhook secret already generated. Reusing it.");
    secret = config.webhookSecret;
  } else {
    secret = randomBytes(32).toString("hex");
    config = { webhookSecret: secret, configuredAt: new Date().toISOString() };
    saveGithubConfig(config);
    console.log("  Generated webhook secret.");
  }

  // Step 2: deploy the Edge Function (requires supabase CLI)
  const hasSupa = supabaseAvailable();
  if (hasSupa) {
    console.log("\n  Deploying Edge Function...");
    try {
      execSync("supabase functions deploy github-webhook", {
        cwd: process.cwd().includes("pcr-dev") ? process.cwd().replace(/pcr-dev.*/, "PCR.dev") : process.cwd(),
        stdio: "inherit",
        timeout: 60000,
      });
      console.log("  Edge Function deployed.");
    } catch {
      console.error("  Edge Function deploy failed. Run manually:");
      console.error("  cd /path/to/PCR.dev && supabase functions deploy github-webhook");
    }

    // Step 3: set the secret in Supabase
    console.log("\n  Setting webhook secret in Supabase...");
    try {
      execSync(`supabase secrets set GITHUB_WEBHOOK_SECRET=${secret}`, {
        stdio: "inherit",
        timeout: 30000,
      });
      console.log("  Secret set.");
    } catch {
      console.error("  Failed to set secret. Run manually:");
      console.error(`  supabase secrets set GITHUB_WEBHOOK_SECRET=${secret}`);
    }
  } else {
    console.log("\n  Supabase CLI not found — run these manually:");
    console.log("  1. cd /path/to/PCR.dev && supabase functions deploy github-webhook");
    console.log(`  2. supabase secrets set GITHUB_WEBHOOK_SECRET=${secret}`);
  }

  // Step 4: print the webhook URL and next steps
  const webhookUrl = getWebhookUrl();
  const settingsUrl = `${PCR_APP_URL}/settings`;

  console.log(`
  ─────────────────────────────────────────────────────

  Webhook URL (add this to your GitHub repo):
  ${webhookUrl}

  Webhook secret (paste this in GitHub's Secret field):
  ${secret}

  ─────────────────────────────────────────────────────

  Next steps:
    1. Go to your GitHub repo → Settings → Webhooks → Add webhook
    2. Paste the URL and secret above
    3. Content type: application/json
    4. Events: select "Pull requests"
    5. Connect GitHub at: ${settingsUrl}
`);
}

async function status(): Promise<void> {
  const config = loadGithubConfig();
  const webhookUrl = getWebhookUrl();

  console.log("\nPCR GitHub integration status\n");

  if (config) {
    console.log(`  Webhook secret:  configured (set ${new Date(config.configuredAt).toLocaleDateString()})`);
  } else {
    console.log("  Webhook secret:  not configured — run `pcr github setup`");
  }

  console.log(`  Webhook URL:     ${webhookUrl}`);
  console.log(`  Connect GitHub:  ${PCR_APP_URL}/settings\n`);
}
