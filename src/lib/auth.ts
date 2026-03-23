import { existsSync, readFileSync, writeFileSync, mkdirSync, unlinkSync } from "fs";
import { join, dirname } from "path";
import { homedir } from "os";
import { PCR_DIR } from "./constants.js";

const AUTH_FILE = join(homedir(), PCR_DIR, "auth.json");

export interface Auth {
  token: string;
  userId: string;
}

export function loadAuth(): Auth | null {
  try {
    if (existsSync(AUTH_FILE)) {
      return JSON.parse(readFileSync(AUTH_FILE, "utf-8")) as Auth;
    }
  } catch {
    // Corrupt or missing
  }
  return null;
}

export function saveAuth(auth: Auth): void {
  const dir = dirname(AUTH_FILE);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  writeFileSync(AUTH_FILE, JSON.stringify(auth, null, 2));
}

export function clearAuth(): void {
  try {
    if (existsSync(AUTH_FILE)) unlinkSync(AUTH_FILE);
  } catch {
    // Best effort
  }
}
