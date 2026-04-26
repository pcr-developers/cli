//! Registered project registry. Direct port of `cli/internal/projects/projects.go`.
//!
//! On-disk format (`$HOME/.pcr-dev/projects.json`) is byte-compatible with
//! the Go version so users can upgrade without re-running `pcr init`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Project {
    pub path: String,
    #[serde(rename = "cursorSlug")]
    pub cursor_slug: String,
    #[serde(rename = "claudeSlug")]
    pub claude_slug: String,
    pub name: String,
    #[serde(rename = "registeredAt")]
    pub registered_at: String,
    #[serde(
        rename = "projectId",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Registry {
    #[serde(default)]
    projects: Vec<Project>,
}

fn file_path() -> PathBuf {
    config::pcr_dir().join("projects.json")
}

/// Registered project list. Cached per-process; the cache is reused
/// while `projects.json`'s mtime is unchanged so the high-frequency
/// watchers don't re-parse it on every poll.
pub fn load() -> Vec<Project> {
    let path = file_path();
    let mtime = mtime_of(&path);
    if let Some(cached) = lookup_cache(mtime) {
        return cached;
    }
    let projects = read_from_disk(&path);
    insert_cache(mtime, &projects);
    projects
}

fn read_from_disk(path: &Path) -> Vec<Project> {
    let Ok(data) = fs::read(path) else {
        return Vec::new();
    };
    let Ok(reg) = serde_json::from_slice::<Registry>(&data) else {
        return Vec::new();
    };
    reg.projects
}

fn save(projects: &[Project]) -> anyhow::Result<()> {
    let path = file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let reg = Registry {
        projects: projects.to_vec(),
    };
    let data = serde_json::to_vec_pretty(&reg)?;
    fs::write(&path, data)?;
    // Drop the cache up-front: some filesystems coalesce mtimes to
    // second resolution and would otherwise serve a stale snapshot.
    invalidate_cache();
    Ok(())
}

struct CacheSlot {
    mtime: Option<SystemTime>,
    projects: Vec<Project>,
}

fn cache() -> &'static Mutex<Option<CacheSlot>> {
    static CACHE: OnceLock<Mutex<Option<CacheSlot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn lookup_cache(current_mtime: Option<SystemTime>) -> Option<Vec<Project>> {
    let guard = cache().lock().ok()?;
    let slot = guard.as_ref()?;
    if slot.mtime == current_mtime {
        Some(slot.projects.clone())
    } else {
        None
    }
}

fn insert_cache(mtime: Option<SystemTime>, projects: &[Project]) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CacheSlot {
            mtime,
            projects: projects.to_vec(),
        });
    }
}

fn invalidate_cache() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

/// `/Users/foo/Desktop/PCR.dev` → `Users-foo-Desktop-PCR-dev`.
/// Matches `projects.PathToCursorSlug`.
pub fn path_to_cursor_slug(path: &str) -> String {
    let mut s = path.trim_start_matches('/').to_string();
    s = s.replace('/', "-");
    s = s.replace('.', "-");
    s
}

/// `/Users/foo/Desktop/PCR.dev` → `-Users-foo-Desktop-PCR.dev`.
/// Matches `projects.PathToClaudeSlug`.
pub fn path_to_claude_slug(path: &str) -> String {
    path.replace('/', "-")
}

pub fn register(project_path: &str) -> Project {
    let mut projects = load();
    let cursor_slug = path_to_cursor_slug(project_path);
    let claude_slug = path_to_claude_slug(project_path);
    let name = Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let existing = projects.iter().position(|p| p.path == project_path);

    let mut entry = Project {
        path: project_path.to_string(),
        cursor_slug,
        claude_slug,
        name,
        registered_at: String::new(),
        project_id: String::new(),
    };

    match existing {
        Some(i) => {
            entry.project_id = projects[i].project_id.clone();
            entry.registered_at = projects[i].registered_at.clone();
            projects[i] = entry.clone();
        }
        None => {
            entry.registered_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            projects.push(entry.clone());
        }
    }

    let _ = save(&projects);
    entry
}

pub fn unregister(project_path: &str) -> bool {
    let mut projects = load();
    let Some(idx) = projects.iter().position(|p| p.path == project_path) else {
        return false;
    };
    projects.remove(idx);
    let _ = save(&projects);
    true
}

pub fn update_project_id(project_path: &str, project_id: &str) {
    let mut projects = load();
    let mut changed = false;
    for p in projects.iter_mut() {
        if p.path == project_path {
            p.project_id = project_id.to_string();
            changed = true;
            break;
        }
    }
    if changed {
        let _ = save(&projects);
    }
}

/// Returns every project that shares `slug` as its cursor slug or lives
/// under a workspace tagged with that slug.
pub fn get_all_projects_for_cursor_slug(slug: &str) -> Vec<Project> {
    load()
        .into_iter()
        .filter(|p| p.cursor_slug == slug || p.cursor_slug.starts_with(&format!("{slug}-")))
        .collect()
}

/// Returns the registered project whose `path` is the deepest prefix of
/// `file_path`. Matches `projects.GetProjectForFile`.
pub fn get_project_for_file<'a>(file_path: &str, candidates: &'a [Project]) -> Option<&'a Project> {
    let mut best: Option<&Project> = None;
    let mut best_len = 0usize;
    for p in candidates {
        let under = file_path.starts_with(&format!("{}/", p.path)) || file_path == p.path;
        if under && p.path.len() > best_len {
            best = Some(p);
            best_len = p.path.len();
        }
    }
    best
}

#[cfg(test)]
mod slug_tests {
    use super::*;

    #[test]
    fn cursor_slug_strips_leading_slash_and_swaps_separators() {
        assert_eq!(
            path_to_cursor_slug("/Users/foo/Desktop/PCR.dev"),
            "Users-foo-Desktop-PCR-dev"
        );
    }

    #[test]
    fn claude_slug_preserves_dots_and_prepends_dash() {
        assert_eq!(
            path_to_claude_slug("/Users/foo/Desktop/PCR.dev"),
            "-Users-foo-Desktop-PCR.dev"
        );
    }
}

pub fn get_project_for_claude_slug(slug: &str) -> Option<Project> {
    let projects = load();
    if let Some(p) = projects.iter().find(|p| p.claude_slug == slug) {
        return Some(p.clone());
    }
    // Ancestor match: slug may refer to a parent of a registered project.
    for p in &projects {
        let mut parent = PathBuf::from(&p.path);
        while let Some(up) = parent.parent() {
            if up.as_os_str().is_empty() || up == Path::new("/") {
                break;
            }
            let cand = path_to_claude_slug(&up.to_string_lossy());
            if cand == slug {
                return Some(p.clone());
            }
            parent = up.to_path_buf();
        }
    }
    None
}
