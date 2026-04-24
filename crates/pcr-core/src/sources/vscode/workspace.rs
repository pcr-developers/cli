//! VS Code workspace discovery. Direct port of
//! `cli/internal/sources/vscode/workspace.go`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::projects::{self, Project};

#[derive(Debug, Clone)]
pub struct WorkspaceMatch {
    pub hash: String,
    pub transcript_dir: PathBuf,
    pub folder_path: PathBuf,
    pub projects: Vec<Project>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct WorkspaceJson {
    folder: String,
    workspace: String,
}

/// `cli/internal/sources/vscode/workspace.go::ScanWorkspaces`.
pub fn scan_workspaces() -> Vec<WorkspaceMatch> {
    let bases = workspace_storage_bases();
    let all_projects = projects::load();
    let mut matches: Vec<WorkspaceMatch> = Vec::new();

    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let hash_dir = entry.path();
            let ws_file = hash_dir.join("workspace.json");
            let Ok(data) = std::fs::read(&ws_file) else {
                continue;
            };
            let Ok(ws) = serde_json::from_slice::<WorkspaceJson>(&data) else {
                continue;
            };
            let Some(folder_path) = resolve_workspace_folder(&ws) else {
                continue;
            };
            let matched = match_projects(&folder_path, &all_projects);
            if matched.is_empty() {
                continue;
            }
            let transcript_dir = hash_dir.join("GitHub.copilot-chat").join("transcripts");
            matches.push(WorkspaceMatch {
                hash: entry.file_name().to_string_lossy().into_owned(),
                transcript_dir,
                folder_path,
                projects: matched,
            });
        }
    }
    matches
}

/// Returns just the base folder path, or None for multi-root workspaces.
fn resolve_workspace_folder(ws: &WorkspaceJson) -> Option<PathBuf> {
    if !ws.folder.is_empty() {
        return Some(PathBuf::from(uri_to_path(&ws.folder)));
    }
    None
}

fn uri_to_path(uri: &str) -> String {
    if !uri.starts_with("file://") {
        return uri.to_string();
    }
    let rest = uri.trim_start_matches("file://");
    // On Windows, `file:///C:/...` — strip the leading slash before the drive letter.
    #[cfg(windows)]
    {
        let bytes = rest.as_bytes();
        if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
            return rest[1..].to_string();
        }
    }
    urlencoding::decode(rest)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| rest.to_string())
}

fn match_projects(workspace_folder: &Path, all: &[Project]) -> Vec<Project> {
    let workspace_str = normalise_path(workspace_folder);
    let mut matched = Vec::new();
    for p in all {
        if p.path.is_empty() {
            continue;
        }
        let proj_path = std::path::Path::new(&p.path);
        let proj_str = normalise_path(proj_path);
        let equals = proj_str == workspace_str;
        let is_child =
            proj_str.starts_with(&format!("{}{}", workspace_str, std::path::MAIN_SEPARATOR));
        if !(equals || is_child) {
            continue;
        }
        if !proj_path.exists() {
            continue;
        }
        matched.push(p.clone());
    }
    matched
}

fn normalise_path(path: &Path) -> String {
    let cleaned: PathBuf = path.components().collect();
    #[cfg(windows)]
    {
        return cleaned.to_string_lossy().to_lowercase();
    }
    #[cfg(not(windows))]
    {
        cleaned.to_string_lossy().into_owned()
    }
}

/// Every platform-appropriate workspaceStorage base, across Code / Insiders / VSCodium.
pub fn workspace_storage_bases() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let config_base = config_base(&home);
    let variants = ["Code", "Code - Insiders", "VSCodium"];
    let mut bases = Vec::new();
    for v in variants {
        let base = config_base.join(v).join("User").join("workspaceStorage");
        if base.is_dir() {
            bases.push(base);
        }
    }
    bases
}

/// Primary single-directory accessor used by the existing Rust code.
pub fn workspace_storage_dir() -> PathBuf {
    // Used by the start banner for display only — return the first existing
    // base or the default Code path.
    let bases = workspace_storage_bases();
    if let Some(first) = bases.first() {
        return first.clone();
    }
    let Some(home) = dirs::home_dir() else {
        return PathBuf::from(".");
    };
    config_base(&home)
        .join("Code")
        .join("User")
        .join("workspaceStorage")
}

/// `~/Library/Application Support/Code/User/globalStorage/`.
pub fn global_storage_base() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    config_base(&home)
        .join("Code")
        .join("User")
        .join("globalStorage")
}

fn config_base(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = home;
        return dirs::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("Library")
            .join("Application Support");
    }
    #[cfg(target_os = "windows")]
    {
        let _ = home;
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata);
        }
        return dirs::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("AppData")
            .join("Roaming");
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home.join(".config")
    }
}
