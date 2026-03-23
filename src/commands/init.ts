import { existsSync } from "fs";
import { execSync } from "child_process";
import { join } from "path";
import { homedir } from "os";
import { registerProject, pathToCursorSlug, updateProjectId } from "../lib/projects.js";
import { loadAuth } from "../lib/auth.js";
import { getSupabase } from "../lib/supabase.js";

export async function runInit(): Promise<void> {
  const projectPath = process.cwd();
  const cursorSlug = pathToCursorSlug(projectPath);
  const cursorProjectDir = join(homedir(), ".cursor", "projects", cursorSlug);

  console.log("\nPCR.dev — initializing project\n");
  console.log(`  Path:  ${projectPath}`);
  console.log(`  Slug:  ${cursorSlug}`);

  if (!existsSync(cursorProjectDir)) {
    console.log(
      `\n  Note: No Cursor project found at ~/.cursor/projects/${cursorSlug}`
    );
    console.log(
      "  This project may not have been opened in Cursor yet — that's fine."
    );
    console.log(
      "  The watcher will start capturing once you open it in Cursor.\n"
    );
  }

  // Register locally first
  const project = registerProject(projectPath);
  console.log(`\n  Registered locally: ${project.name}`);

  // Try to detect git remote URL
  let repoUrl: string | undefined;
  try {
    const remote = execSync("git remote get-url origin", {
      encoding: "utf-8",
      cwd: projectPath,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
    if (remote) repoUrl = remote;
  } catch {
    // No git remote — fine
  }

  // Register remotely if logged in
  const auth = loadAuth();
  if (!auth) {
    console.log(
      "\n  Not logged in — project registered locally only."
    );
    console.log("  Run `pcr login` to sync this project to your dashboard.\n");
  } else {
    try {
      const { data: projectId, error } = await getSupabase().rpc("register_project", {
        p_token: auth.token,
        p_name: project.name,
        p_slug: cursorSlug,
        p_repo_url: repoUrl ?? null,
      });

      if (error || !projectId) {
        console.log(
          `\n  Remote registration failed: ${error?.message ?? "unknown error"}`
        );
        console.log("  Project is registered locally only.\n");
      } else {
        updateProjectId(projectPath, projectId as string);
        console.log(`  Synced to dashboard (project ID: ${projectId})`);
        if (repoUrl) {
          console.log(`  Repo: ${repoUrl}`);
        }
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      console.log(`\n  Remote registration failed: ${msg}`);
      console.log("  Project is registered locally only.\n");
    }
  }

  console.log(
    "\n  The watcher will only capture prompts from this project.\n"
  );
  console.log("  Next steps:");
  if (!auth) {
    console.log("    pcr login    — authenticate to sync to your dashboard");
  }
  console.log("    pcr start    — start the watcher\n");
}
