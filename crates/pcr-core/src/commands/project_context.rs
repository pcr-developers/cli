//! Resolves the registered projects relevant to the current working
//! directory. Mirrors `cli/cmd/project_context.go` one-for-one.

use std::path::PathBuf;

use crate::projects::{self, Project};
use crate::sources::shared::git;

#[derive(Debug, Default, Clone)]
pub struct ProjectContext {
    /// Display name for the header (innermost matched project).
    pub name: String,
    /// Project IDs used when filtering store queries.
    pub ids: Vec<String>,
    /// Project names (and claude slugs) used when filtering store queries.
    pub names: Vec<String>,
    /// True when cwd is exactly a registered project path.
    pub single_repo: bool,
}

pub fn resolve() -> ProjectContext {
    let mut cwd = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .into_owned();
    let git_root = git::git_output(&["rev-parse", "--show-toplevel"]);
    if !git_root.is_empty() {
        cwd = git_root;
    }

    let projs = projects::load();

    // Exact match first — strict single-repo scoping.
    for p in &projs {
        if p.path == cwd {
            let mut ctx = ProjectContext {
                name: p.name.clone(),
                single_repo: true,
                ..Default::default()
            };
            if !p.project_id.is_empty() {
                ctx.ids = vec![p.project_id.clone()];
            }
            if !p.name.is_empty() {
                ctx.names.push(p.name.clone());
            }
            if !p.claude_slug.is_empty() {
                ctx.names.push(p.claude_slug.clone());
            }
            return ctx;
        }
    }

    let mut ctx = ProjectContext::default();
    let mut best_len = 0usize;
    let mut seen = std::collections::HashSet::<String>::new();
    for p in &projs {
        let prefix = format!("{}/", cwd);
        if !p.path.starts_with(&prefix) {
            continue;
        }
        if !seen.insert(p.path.clone()) {
            continue;
        }
        if p.path.len() > best_len {
            ctx.name = p.name.clone();
            best_len = p.path.len();
        }
        if !p.project_id.is_empty() {
            ctx.ids.push(p.project_id.clone());
        }
        if !p.name.is_empty() {
            ctx.names.push(p.name.clone());
        }
        if !p.claude_slug.is_empty() {
            ctx.names.push(p.claude_slug.clone());
        }
    }
    ctx
}

/// `projByID` lookup from [`cli/cmd/bundle.go::loadProjByID`].
pub fn load_proj_by_id() -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    for p in projects::load() {
        if !p.project_id.is_empty() {
            m.insert(p.project_id, p.name);
        }
    }
    m
}

pub fn _unused_project(p: &Project) -> &Project {
    p
}
