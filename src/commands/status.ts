import { loadAuth } from "../lib/auth.js";
import { loadProjects } from "../lib/projects.js";

export async function runStatus(): Promise<void> {
  const auth = loadAuth();
  const projects = loadProjects();

  console.log("\nPCR.dev status\n");

  if (auth) {
    console.log(`  Auth:     logged in as ${auth.userId}`);
  } else {
    console.log("  Auth:     not logged in  (run: pcr login)");
  }

  if (projects.length === 0) {
    console.log("  Projects: none registered  (run: pcr init in a project directory)");
  } else {
    console.log(`  Projects: ${projects.length} registered`);
    for (const p of projects) {
      const registered = new Date(p.registeredAt).toLocaleDateString();
      console.log(`    - ${p.name}  (${p.path})  [since ${registered}]`);
    }
  }

  console.log();
}
