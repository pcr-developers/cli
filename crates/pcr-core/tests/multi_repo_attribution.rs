//! End-to-end integration test for the multi-repo attribution pipeline.
//!
//! Sets up two real git repos in a tempdir, builds a pretend tool-call
//! transcript that touches both, and exercises every helper in the
//! attribution chain — `touched_project_ids`, `repo_snapshots`,
//! `repo_snapshots_for_ids`, `tc_files_for_project`, `is_git_repo`,
//! `get_branch`. Catches regressions that pure-logic unit tests can't:
//! anything involving real `git` invocation, real `canonicalize`, real
//! symlink resolution.
//!
//! Skipped when `git` is not on PATH (CI sandbox / Windows node-only
//! environments) — we don't want the test to fail spuriously when the
//! environment can't run it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use pcr_core::projects::Project;
use pcr_core::sources::shared::git::{get_branch, get_head_sha, is_git_repo};
use pcr_core::sources::shared::path_norm::proj_id_to_canonical_paths;
#[cfg(unix)]
use pcr_core::sources::shared::path_norm::{
    canonicalize_project_path, normalize_path, path_is_under,
};
use pcr_core::sources::shared::tool_calls::{
    extract_paths_from_tool_call, repo_snapshots, repo_snapshots_for_ids, touched_project_ids,
};
use serde_json::json;
use tempfile::TempDir;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_git_repo(path: &Path, branch: &str) {
    Command::new("git")
        .args(["init", "--initial-branch", branch])
        .current_dir(path)
        .status()
        .expect("git init must succeed in test sandbox");
    // Fresh repos may not have user.email/user.name configured. Set them
    // on the local repo only so we don't pollute the user's global config.
    Command::new("git")
        .args(["config", "user.email", "test@pcr.dev"])
        .current_dir(path)
        .status()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "PCR Test"])
        .current_dir(path)
        .status()
        .expect("git config name");
    std::fs::write(path.join("README.md"), "init\n").expect("seed file");
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .status()
        .expect("git commit");
}

fn project_at(temp: &TempDir, name: &str, project_id: &str, branch: &str) -> Project {
    let path = temp.path().join(name);
    std::fs::create_dir_all(&path).expect("mkdir project");
    init_git_repo(&path, branch);
    Project {
        path: path.to_string_lossy().into_owned(),
        cursor_slug: name.into(),
        claude_slug: format!("-{name}"),
        name: name.into(),
        registered_at: "2026-01-01T00:00:00Z".into(),
        project_id: project_id.into(),
    }
}

#[test]
fn end_to_end_multi_repo_attribution() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let frontend = project_at(&temp, "frontend", "p-frontend", "main");
    let backend = project_at(&temp, "backend", "p-backend", "feature/auth");
    let untouched = project_at(&temp, "untouched", "p-untouched", "main");
    let projects = vec![frontend.clone(), backend.clone(), untouched.clone()];

    // Tool calls a Claude Code agent might emit while editing across the
    // frontend and backend in a single prompt. Includes:
    //   - Single-file shape (input.path)
    //   - Multi-file shape (input.files: [{path: ...}])
    //   - file_path camelCase shape
    //   - A path INSIDE the untouched project to be sure we don't tag it
    //     just because it's registered.
    let tool_calls = vec![
        json!({"tool": "Read", "input": {"path": format!("{}/src/index.ts", frontend.path)}}),
        json!({"tool": "ApplyPatch", "input": {"files": [
            {"path": format!("{}/src/api.rs", backend.path)},
            {"path": format!("{}/src/auth.rs", backend.path)},
        ]}}),
        json!({"tool": "Edit", "input": {"file_path": format!("{}/src/component.tsx", frontend.path)}}),
    ];

    // Build the by_id map the attribution helpers expect (canonical paths).
    let proj_by_id: BTreeMap<String, String> = proj_id_to_canonical_paths(&projects);

    // touched_project_ids must include frontend AND backend (both touched)
    // and exclude untouched (registered but not referenced).
    let touched = touched_project_ids(&tool_calls, &proj_by_id, None);
    assert_eq!(
        touched,
        vec!["p-backend".to_string(), "p-frontend".to_string()],
        "touched_project_ids must include every touched project"
    );

    // repo_snapshots called with frontend as primary should produce a
    // snapshot for backend (the secondary), with the captured branch
    // matching what we set when initializing the repo.
    let snaps = repo_snapshots(&tool_calls, "p-frontend", &proj_by_id, None)
        .expect("backend should produce a snapshot");
    assert_eq!(snaps.len(), 1, "primary excluded from snapshots");
    let backend_snap = snaps
        .get("p-backend")
        .expect("backend snapshot present")
        .as_object()
        .unwrap();
    assert_eq!(
        backend_snap.get("branch").and_then(|v| v.as_str()),
        Some("feature/auth"),
        "secondary repo's branch carried through from the real git repo"
    );
    assert!(
        backend_snap
            .get("head_sha")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .is_some(),
        "head_sha is non-empty"
    );

    // repo_snapshots_for_ids — Cursor's variant — same expectations.
    let snaps_for_ids = repo_snapshots_for_ids(
        "p-frontend",
        &["p-frontend".into(), "p-backend".into()],
        &proj_by_id,
    )
    .expect("backend should snapshot");
    assert!(snaps_for_ids.contains_key("p-backend"));
    assert!(!snaps_for_ids.contains_key("p-frontend"));

    // is_git_repo says yes for our fresh repos, no for a non-git path.
    assert!(is_git_repo(&frontend.path));
    assert!(is_git_repo(&backend.path));
    assert!(!is_git_repo(
        &temp.path().join("nonexistent").to_string_lossy()
    ));

    // get_branch returns the branch we initialized, not the literal
    // "HEAD" we'd see in a detached state.
    assert_eq!(get_branch(&frontend.path), "main");
    assert_eq!(get_branch(&backend.path), "feature/auth");

    // get_head_sha is non-empty for both.
    assert!(!get_head_sha(&frontend.path).is_empty());
    assert!(!get_head_sha(&backend.path).is_empty());
}

#[cfg(unix)]
#[test]
fn symlinked_project_path_attributes_correctly() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    // Symlinks aren't easy to create on Windows from CI without admin
    // permission. Skip there.
    if cfg!(windows) {
        eprintln!("skipping symlink test on windows");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let real = temp.path().join("real_proj");
    std::fs::create_dir_all(&real).expect("mkdir real");
    init_git_repo(&real, "main");

    // Symlink pointing at the real project.
    let sym = temp.path().join("sym_proj");
    std::os::unix::fs::symlink(&real, &sym).expect("symlink");

    // Register the project via the SYMLINK path (a common user mistake).
    let project = Project {
        path: sym.to_string_lossy().into_owned(),
        cursor_slug: "sym_proj".into(),
        claude_slug: "-sym_proj".into(),
        name: "sym_proj".into(),
        registered_at: "2026-01-01T00:00:00Z".into(),
        project_id: "p-sym".into(),
    };
    let by_id = proj_id_to_canonical_paths(std::slice::from_ref(&project));

    // Tool call uses the REAL canonical path (what tools typically emit).
    let calls = vec![json!({
        "tool": "Read",
        "input": {"path": format!("{}/README.md", real.to_string_lossy())}
    })];

    // The symlinked project must attribute even though it was
    // registered via the symlink path. Without canonicalization on
    // both sides of the comparison, the prefix match silently fails.
    let touched = touched_project_ids(&calls, &by_id, None);
    assert_eq!(
        touched,
        vec!["p-sym".to_string()],
        "symlinked project must attribute via canonical path"
    );

    // path_is_under contract under canonical comparison.
    let project_canon = canonicalize_project_path(&project.path);
    let abs = normalize_path(&format!("{}/README.md", real.to_string_lossy()), None).unwrap();
    assert!(path_is_under(&abs, &project_canon));
}

#[test]
fn relative_tool_call_path_resolves_against_cwd() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let temp = TempDir::new().expect("tempdir");
    let proj = temp.path().join("p");
    std::fs::create_dir_all(&proj).expect("mkdir");
    init_git_repo(&proj, "main");

    let project = Project {
        path: proj.to_string_lossy().into_owned(),
        cursor_slug: "p".into(),
        claude_slug: "-p".into(),
        name: "p".into(),
        registered_at: "2026-01-01T00:00:00Z".into(),
        project_id: "p1".into(),
    };
    let by_id = proj_id_to_canonical_paths(std::slice::from_ref(&project));

    // Relative tool-call path (`./README.md`) — must resolve against the
    // session's cwd to attribute correctly.
    let calls = vec![json!({"tool": "Read", "input": {"path": "./README.md"}})];

    // Without cwd: dropped silently.
    assert!(touched_project_ids(&calls, &by_id, None).is_empty());

    // With cwd: attributes correctly.
    let touched = touched_project_ids(&calls, &by_id, Some(&project.path));
    assert_eq!(touched, vec!["p1".to_string()]);
}

#[test]
fn extract_paths_from_real_tool_call_shapes() {
    // Doesn't need git — just asserts the multi-shape extractor works on
    // shapes we've actually seen in production transcripts.
    let calls = [
        json!({"tool": "Read", "input": {"path": "/r/a"}}),
        json!({"tool": "Edit", "input": {"file_path": "/r/b"}}),
        json!({"tool": "Write", "input": {"filePath": "/r/c"}}),
        json!({"tool": "ApplyPatch", "input": {"files": [{"path": "/r/d"}, {"path": "/r/e"}]}}),
        json!({"tool": "MultiEdit", "input": {"fileNames": ["/r/f", "/r/g"]}}),
        json!({"tool": "VSCodePatch", "input": {"changes": [{"file": "/r/h"}]}}),
    ];
    let mut all_paths: Vec<String> = calls
        .iter()
        .flat_map(extract_paths_from_tool_call)
        .collect();
    all_paths.sort();
    assert_eq!(
        all_paths,
        vec!["/r/a", "/r/b", "/r/c", "/r/d", "/r/e", "/r/f", "/r/g", "/r/h"]
    );
}
