/**
 * pcr github — set up GitHub PR integration.
 *
 * Usage:
 *   pcr github setup    — generates webhook secret, deploys Edge Function,
 *                         sets the secret, and creates the webhook on GitHub
 *   pcr github status   — shows whether GitHub is connected and the webhook URL
 */

import { randomBytes } from "crypto";
import { execSync, spawnSync } from "child_process";
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "fs";
import { join, dirname } from "path";
import { homedir, platform } from "os";
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

function getProjectRef(): string {
  return PCR_SUPABASE_URL.replace("https://", "").replace(".supabase.co", "");
}

function getWebhookUrl(): string {
  return `https://${getProjectRef()}.supabase.co/functions/v1/github-webhook`;
}

function supabaseAvailable(): boolean {
  return spawnSync("supabase", ["--version"], { stdio: "pipe" }).status === 0;
}

function ghAvailable(): boolean {
  return spawnSync("gh", ["--version"], { stdio: "pipe" }).status === 0;
}

/** Parse owner/repo from any GitHub remote URL format. */
function getRepoFullName(): string | null {
  try {
    const remote = execSync("git remote get-url origin", {
      encoding: "utf-8",
      cwd: process.cwd(),
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
    const match = remote.match(/github\.com[:/]([^/]+\/[^/.]+)/);
    return match ? match[1] : null;
  } catch {
    return null;
  }
}

/**
 * Find the supabase project root that contains the github-webhook function.
 * The Supabase CLI expects source at supabase/functions/<name>/index.ts.
 * Searches the current directory and sibling/parent directories so this works
 * regardless of which repo the user runs `pcr github setup` from.
 */
function findFunctionsDir(): string | null {
  const cwd = process.cwd();
  const candidates = [
    cwd,
    join(cwd, ".."),
    join(cwd, "..", "functions"),
    join(cwd, "functions"),
  ];
  for (const dir of candidates) {
    if (existsSync(join(dir, "supabase", "functions", "github-webhook", "index.ts"))) {
      return dir;
    }
  }
  return null;
}

/** Create the webhook on GitHub via the `gh` CLI. Returns true on success. */
function createWebhookViaGh(repoFullName: string, webhookUrl: string, secret: string): boolean {
  try {
    const payload = JSON.stringify({
      name: "web",
      active: true,
      events: ["pull_request"],
      config: { url: webhookUrl, content_type: "json", secret, insecure_ssl: "0" },
    });
    execSync(`gh api repos/${repoFullName}/hooks --method POST --input -`, {
      input: payload,
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    });
    return true;
  } catch (err: unknown) {
    const stderr = err instanceof Error ? (err as NodeJS.ErrnoException & { stderr?: Buffer }).stderr?.toString() ?? err.message : String(err);
    // 422 = webhook with this URL already exists
    if (stderr.includes("422") || stderr.includes("already exists")) {
      return true;
    }
    console.error(`  gh error: ${stderr.trim()}`);
    return false;
  }
}

/** Open a URL in the default browser (macOS / Linux / Windows). */
function openBrowser(url: string): void {
  const cmd = platform() === "darwin" ? "open" : platform() === "win32" ? "start" : "xdg-open";
  try { spawnSync(cmd, [url], { stdio: "pipe" }); } catch { /* ignore */ }
}

export async function runGithub(subcommand?: string): Promise<void> {
  const cmd = subcommand ?? "status";
  switch (cmd) {
    case "setup":  await setup();  break;
    case "status": await status(); break;
    default:
      console.log(`
pcr github — GitHub PR integration

Usage:
  pcr github setup    Set up the webhook (deploys Edge Function, creates webhook)
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

  const webhookUrl = getWebhookUrl();
  const projectRef = getProjectRef();

  // Step 2: deploy the Edge Function
  if (supabaseAvailable()) {
    const functionsDir = findFunctionsDir();
    if (functionsDir) {
      console.log("\n  Deploying Edge Function...");
      try {
        execSync(`supabase functions deploy github-webhook --project-ref ${projectRef}`, {
          cwd: functionsDir,
          stdio: "inherit",
          timeout: 60000,
        });
        console.log("  Edge Function deployed.");
      } catch {
        console.error("  Deploy failed — the function may already be up to date.");
      }
    } else {
      console.log("\n  Could not locate function source — skipping deploy.");
      console.log(`  To deploy manually, run from the functions/ directory:`);
      console.log(`  supabase functions deploy github-webhook --project-ref ${projectRef}`);
    }

    // Step 3: set the secret in Supabase Vault
    console.log("\n  Setting webhook secret in Supabase...");
    try {
      execSync(`supabase secrets set GITHUB_WEBHOOK_SECRET=${secret} --project-ref ${projectRef}`, {
        stdio: "inherit",
        timeout: 30000,
      });
      console.log("  Secret set.");
    } catch {
      console.error("  Failed to set secret. Run manually:");
      console.error(`  supabase secrets set GITHUB_WEBHOOK_SECRET=${secret} --project-ref ${projectRef}`);
    }
  } else {
    console.log("\n  Supabase CLI not found — skipping deploy and secret set.");
    console.log(`  Run these manually from the functions/ directory:`);
    console.log(`  supabase functions deploy github-webhook --project-ref ${projectRef}`);
    console.log(`  supabase secrets set GITHUB_WEBHOOK_SECRET=${secret} --project-ref ${projectRef}`);
  }

  // Step 4: create the webhook on GitHub automatically
  const repoFullName = getRepoFullName();
  console.log("\n  Setting up GitHub webhook...");

  if (!repoFullName) {
    console.log("  Could not detect GitHub repo from git remote.");
  } else if (ghAvailable()) {
    console.log(`  Creating webhook on ${repoFullName}...`);
    const ok = createWebhookViaGh(repoFullName, webhookUrl, secret);
    if (ok) {
      console.log(`  Webhook created on github.com/${repoFullName}`);
    } else {
      console.log("  gh API call failed — opening GitHub in your browser instead.");
      openBrowser(`https://github.com/${repoFullName}/settings/hooks/new`);
    }
  } else {
    console.log("  gh CLI not found — opening GitHub in your browser.");
    if (repoFullName) openBrowser(`https://github.com/${repoFullName}/settings/hooks/new`);
  }

  console.log(`
  ─────────────────────────────────────────────────────

  Webhook URL:    ${webhookUrl}
  Webhook secret: ${secret}

  ─────────────────────────────────────────────────────

  Last step: connect your GitHub account at ${PCR_APP_URL}/settings
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
