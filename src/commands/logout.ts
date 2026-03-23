import { loadAuth, clearAuth } from "../lib/auth.js";

export async function runLogout(): Promise<void> {
  const auth = loadAuth();

  if (!auth) {
    console.log("\nNot logged in.\n");
    return;
  }

  clearAuth();
  console.log(`\nLogged out (was: ${auth.userId})\n`);
}
