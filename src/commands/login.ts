import * as readline from "readline/promises";
import { exec } from "child_process";
import { getSupabase } from "../lib/supabase.js";
import { saveAuth, loadAuth } from "../lib/auth.js";
import { PCR_APP_URL } from "../lib/constants.js";

function openBrowser(url: string): void {
  const cmd =
    process.platform === "darwin"
      ? "open"
      : process.platform === "win32"
      ? "start"
      : "xdg-open";
  exec(`${cmd} "${url}"`);
}

export async function runLogin(): Promise<void> {
  const existing = loadAuth();
  if (existing) {
    console.log(`\nAlready logged in as ${existing.userId}`);
    console.log("Run `pcr logout` first to switch accounts.\n");
    return;
  }

  const settingsUrl = `${PCR_APP_URL}/settings`;

  console.log("\nPCR.dev — login\n");
  console.log(`Opening: ${settingsUrl}\n`);
  console.log("Steps:");
  console.log("  1. Sign in to the PCR.dev dashboard");
  console.log("  2. Go to Settings");
  console.log('  3. Click "New CLI Token", give it a name, copy the token\n');

  openBrowser(settingsUrl);

  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  const token = (await rl.question("Paste your CLI token: ")).trim();
  rl.close();

  if (!token) {
    console.error("\nNo token provided. Aborting.\n");
    process.exit(1);
  }

  const { data: userId, error } = await getSupabase().rpc("validate_cli_token", {
    p_token: token,
  });

  if (error || !userId) {
    console.error(
      "\nInvalid or expired token. Create a new one in Settings.\n"
    );
    process.exit(1);
  }

  saveAuth({ token, userId: userId as string });

  console.log(`\nLogged in. User ID: ${userId}`);
  console.log("\nPrompts captured by the watcher will be tagged with your account.\n");
  console.log("Next steps:");
  console.log("  pcr init     — register a project to track");
  console.log("  pcr start    — start the watcher\n");
}
