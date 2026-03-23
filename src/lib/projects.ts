import { existsSync, readFileSync, writeFileSync, mkdirSync } from "fs";
import { join, dirname, basename } from "path";
import { homedir } from "os";
import { PCR_DIR } from "./constants.js";

const PROJECTS_FILE = join(homedir(), PCR_DIR, "projects.json");

export interface Project {
  path: string;
  cursorSlug: string;
  claudeSlug: string;
  name: string;
  registeredAt: string;
  projectId?: string; // Supabase UUID, set after remote registration via pcr init
}

interface ProjectsRegistry {
  projects: Project[];
}

export function loadProjects(): Project[] {
  try {
    if (existsSync(PROJECTS_FILE)) {
      const data = JSON.parse(readFileSync(PROJECTS_FILE, "utf-8")) as ProjectsRegistry;
      return data.projects ?? [];
    }
  } catch {
    // Corrupt or missing
  }
  return [];
}

function saveProjects(projects: Project[]): void {
  const dir = dirname(PROJECTS_FILE);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  writeFileSync(PROJECTS_FILE, JSON.stringify({ projects }, null, 2));
}

/**
 * Cursor derives project slugs by taking the absolute workspace path,
 * stripping the leading slash, and replacing all "/" and "." with "-".
 * e.g. /Users/kalujo/Desktop/PCR.dev -> Users-kalujo-Desktop-PCR-dev
 */
export function pathToCursorSlug(projectPath: string): string {
  return projectPath.replace(/^\//, "").replace(/[/.]/g, "-");
}

/**
 * Claude Code derives project slugs by taking the absolute workspace path,
 * stripping the leading slash, and replacing all "/" with "-".
 * e.g. /Users/kalujo/Desktop/PCR.dev -> Users-kalujo-Desktop-PCR.dev
 */
export function pathToClaudeSlug(projectPath: string): string {
  return projectPath.replace(/^\//, "").replace(/\//g, "-");
}

export function registerProject(projectPath: string): Project {
  const projects = loadProjects();
  const cursorSlug = pathToCursorSlug(projectPath);
  const claudeSlug = pathToClaudeSlug(projectPath);
  const name = basename(projectPath);

  const existing = projects.findIndex((p) => p.path === projectPath);
  const entry: Project = {
    path: projectPath,
    cursorSlug,
    claudeSlug,
    name,
    // Preserve existing projectId and registeredAt if already registered
    projectId: existing >= 0 ? projects[existing].projectId : undefined,
    registeredAt: existing >= 0 ? projects[existing].registeredAt : new Date().toISOString(),
  };

  if (existing >= 0) {
    projects[existing] = entry;
  } else {
    projects.push(entry);
  }

  saveProjects(projects);
  return entry;
}

export function unregisterProject(projectPath: string): boolean {
  const projects = loadProjects();
  const idx = projects.findIndex((p) => p.path === projectPath);
  if (idx < 0) return false;
  projects.splice(idx, 1);
  saveProjects(projects);
  return true;
}

export function getRegisteredCursorSlugs(): Set<string> {
  return new Set(loadProjects().map((p) => p.cursorSlug));
}

export function getRegisteredClaudeSlugs(): Set<string> {
  return new Set(loadProjects().map((p) => p.claudeSlug));
}

export function getProjectIdForCursorSlug(slug: string): string | undefined {
  return loadProjects().find((p) => p.cursorSlug === slug)?.projectId;
}

export function getProjectPathForCursorSlug(slug: string): string | undefined {
  return loadProjects().find((p) => p.cursorSlug === slug)?.path;
}

export function getProjectIdForClaudeSlug(slug: string): string | undefined {
  return loadProjects().find((p) => p.claudeSlug === slug)?.projectId;
}

export function updateProjectId(projectPath: string, projectId: string): void {
  const projects = loadProjects();
  const idx = projects.findIndex((p) => p.path === projectPath);
  if (idx >= 0) {
    projects[idx].projectId = projectId;
    const dir = dirname(PROJECTS_FILE);
    if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
    writeFileSync(PROJECTS_FILE, JSON.stringify({ projects }, null, 2));
  }
}
