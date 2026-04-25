//! Path normalization helpers for tool-call → project attribution.
//!
//! Tool calls captured from Cursor / Claude Code / VS Code arrive as raw
//! file path strings. The naive prefix-match (`path.starts_with(project +
//! "/")`) used to fail in three real-world ways:
//!
//! 1. **Symlinks.** A user opens `/Users/me/dev/proj` (a symlink to
//!    `/Volumes/Data/me/dev/proj`). The tool call uses the canonical path,
//!    the registered project uses the symlink path — no match.
//! 2. **Relative paths.** A tool call emits `./src/main.rs` (some tools
//!    do this for files mentioned by the user). No prefix match against an
//!    absolute project path.
//! 3. **`~`-prefixed paths.** A tool call emits `~/code/proj/foo.rs`. Same
//!    failure mode.
//!
//! The fix is to normalize both sides to the same canonical form before
//! comparing. We do this lazily and best-effort:
//!
//! - `normalize_path(raw, base_cwd)` turns a tool-call string into an
//!   absolute, symlink-resolved path. Falls back to the textually-cleaned
//!   absolute path when the file no longer exists (deleted/renamed since
//!   capture).
//! - `canonicalize_project_path(p)` does the same for a registered project
//!   path. Falls back to the literal path on failure so attribution still
//!   works for projects on a network mount that's currently offline.
//! - `path_is_under(abs, project_canonical)` does the comparison itself,
//!   handling exact-equality and trailing-slash edge cases.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::projects::Project;

/// Normalize a raw tool-call path into an absolute, symlink-resolved form.
///
/// Returns `None` only when the input is empty or genuinely cannot be made
/// absolute (relative path with no `base_cwd`). Every other case produces
/// some form of absolute path so attribution always has a chance to match.
pub fn normalize_path(raw: &str, base_cwd: Option<&str>) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. Expand `~/` and bare `~`. We only consume HOME-prefix tildes —
    // `~user` (other-user expansion) is rare and platform-specific, and we
    // don't try to resolve it.
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = dirs::home_dir()?;
        home.join(rest).to_string_lossy().into_owned()
    } else if trimmed == "~" {
        dirs::home_dir()?.to_string_lossy().into_owned()
    } else {
        trimmed.to_string()
    };

    // 2. Resolve relative paths against base_cwd if provided.
    let abs_string = if Path::new(&expanded).is_absolute() {
        expanded
    } else {
        match base_cwd.filter(|s| !s.is_empty()) {
            Some(cwd) => Path::new(cwd)
                .join(&expanded)
                .to_string_lossy()
                .into_owned(),
            None => return None, // relative path with no cwd → can't resolve
        }
    };

    // 3. Try canonicalize. This both verifies existence and resolves
    // symlinks. If the file was deleted / renamed between capture and
    // lookup, fall back to a textually-cleaned absolute path so we still
    // get attribution against the registered project (the path STARTED
    // valid; the project_id is what matters).
    if let Ok(canon) = std::fs::canonicalize(&abs_string) {
        return Some(canon.to_string_lossy().into_owned());
    }
    Some(clean_path(&abs_string))
}

/// Canonicalize a registered project path. Falls back to the literal path
/// on failure (network mount offline, project moved on disk, etc.) so we
/// still produce an attribution key — just one that won't catch
/// symlink-aliased tool calls until the project is reachable again.
pub fn canonicalize_project_path(project_path: &str) -> String {
    if project_path.is_empty() {
        return String::new();
    }
    std::fs::canonicalize(project_path)
        .ok()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| project_path.to_string())
}

/// True when `abs_path` is the project directory itself or lies strictly
/// below it. Both inputs MUST already be canonical / absolute or the
/// result is meaningless.
///
/// We hand-implement the prefix check rather than using
/// `Path::starts_with` because the latter requires component equality —
/// `/foo/bar/` doesn't `starts_with("/foo/bar/baz")` even though it
/// should not match. We need the byte-string-level "X/" or "X" check.
pub fn path_is_under(abs_path: &str, project_canonical_path: &str) -> bool {
    if project_canonical_path.is_empty() {
        return false;
    }
    if abs_path == project_canonical_path {
        return true;
    }
    let with_sep = format!("{project_canonical_path}/");
    abs_path.starts_with(&with_sep)
}

/// Strip a project prefix from a canonical absolute path, returning the
/// repo-relative path. Returns `None` when the path isn't actually under
/// the project. The caller is responsible for ensuring both inputs are
/// canonical.
pub fn strip_project_prefix<'a>(
    abs_path: &'a str,
    project_canonical_path: &str,
) -> Option<&'a str> {
    if abs_path == project_canonical_path {
        return Some("");
    }
    let with_sep = format!("{project_canonical_path}/");
    abs_path.strip_prefix(&with_sep)
}

/// Build a `project_id → canonical_path` map for use by the attribution
/// helpers. Skips projects without a `project_id` (not yet linked to
/// Supabase) or without a path.
pub fn proj_id_to_canonical_paths(projects: &[Project]) -> BTreeMap<String, String> {
    projects
        .iter()
        .filter(|p| !p.project_id.is_empty() && !p.path.is_empty())
        .map(|p| (p.project_id.clone(), canonicalize_project_path(&p.path)))
        .collect()
}

/// Like `proj_id_to_canonical_paths` but yields `(canonical_path, &Project)`
/// pairs so callers that need the full project (not just the ID) don't
/// have to do a second lookup.
pub fn projects_by_canonical_path(projects: &[Project]) -> Vec<(String, Project)> {
    projects
        .iter()
        .filter(|p| !p.path.is_empty())
        .map(|p| (canonicalize_project_path(&p.path), (*p).clone()))
        .collect()
}

// ─── Internal helpers ───────────────────────────────────────────────────────

/// Resolve `.` and `..` segments in `s` without touching the filesystem.
/// Used as a fallback when canonicalize fails.
fn clean_path(s: &str) -> String {
    let pb = PathBuf::from(s);
    let mut stack: Vec<PathBuf> = Vec::new();
    for comp in pb.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                stack.push(PathBuf::from(comp.as_os_str()));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop unless we're already at the root (one entry is the
                // root prefix on Unix, e.g. `/`).
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            Component::Normal(s) => {
                stack.push(PathBuf::from(s));
            }
        }
    }
    let mut out = PathBuf::new();
    for seg in stack {
        out.push(seg);
    }
    out.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(normalize_path("", None), None);
        assert_eq!(normalize_path("   ", None), None);
    }

    #[test]
    fn relative_with_no_cwd_returns_none() {
        assert_eq!(normalize_path("./src/main.rs", None), None);
        assert_eq!(normalize_path("src/main.rs", None), None);
    }

    #[test]
    fn relative_path_resolves_against_cwd() {
        // We don't depend on the FS existing — `clean_path` fallback handles
        // it. The .canonicalize() call will fail and we fall through.
        let got = normalize_path("./src/main.rs", Some("/tmp/proj")).unwrap();
        // On macOS /tmp may itself be a symlink to /private/tmp, but only if
        // the path exists. For the non-existent case we use clean_path.
        assert!(got.ends_with("/proj/src/main.rs") || got.ends_with("proj/src/main.rs"));
    }

    #[test]
    fn parent_segments_collapse_in_clean_path() {
        // "src/../lib/foo.rs" → "lib/foo.rs"
        assert_eq!(clean_path("/proj/src/../lib/foo.rs"), "/proj/lib/foo.rs");
        // ".." past root stays at root
        assert_eq!(clean_path("/../foo.rs"), "/foo.rs");
        // "." segments dropped
        assert_eq!(clean_path("/proj/./foo.rs"), "/proj/foo.rs");
    }

    #[test]
    fn tilde_expands_to_home() {
        let got = normalize_path("~/foo.rs", None);
        assert!(got.is_some(), "tilde expansion should produce a path");
        let got = got.unwrap();
        assert!(!got.starts_with('~'), "~ should be expanded, got {got:?}");
    }

    #[test]
    fn bare_tilde_expands_to_home() {
        let got = normalize_path("~", None).expect("bare tilde");
        assert!(
            !got.starts_with('~'),
            "bare ~ should be expanded, got {got:?}"
        );
        assert!(Path::new(&got).is_absolute());
    }

    #[test]
    fn absolute_paths_pass_through() {
        // Non-existent absolute path — clean_path fallback returns it as-is.
        let got = normalize_path("/definitely/not/a/real/path/foo.rs", None).unwrap();
        assert_eq!(got, "/definitely/not/a/real/path/foo.rs");
    }

    #[test]
    fn whitespace_around_input_is_trimmed() {
        let got = normalize_path("  /proj/foo.rs  ", None).unwrap();
        assert_eq!(got, "/proj/foo.rs");
    }

    #[test]
    fn path_is_under_handles_exact_equality() {
        assert!(path_is_under("/proj", "/proj"));
        assert!(path_is_under("/proj/src/main.rs", "/proj"));
    }

    #[test]
    fn path_is_under_does_not_false_match_partial_segments() {
        // "/proj-other" must NOT match "/proj" — common bug when matching
        // by string prefix without the trailing slash.
        assert!(!path_is_under("/proj-other/foo", "/proj"));
        assert!(!path_is_under("/projects", "/proj"));
    }

    #[test]
    fn path_is_under_handles_empty_project() {
        assert!(!path_is_under("/proj/foo", ""));
    }

    #[test]
    fn strip_project_prefix_handles_root_and_subdir() {
        assert_eq!(strip_project_prefix("/proj", "/proj"), Some(""));
        assert_eq!(
            strip_project_prefix("/proj/src/main.rs", "/proj"),
            Some("src/main.rs")
        );
        assert_eq!(strip_project_prefix("/other/foo", "/proj"), None);
    }

    #[test]
    fn proj_id_to_canonical_paths_skips_unlinked_or_pathless() {
        let projects = vec![
            Project {
                project_id: "p1".into(),
                path: "/tmp/p1".into(),
                ..Default::default()
            },
            Project {
                project_id: "".into(), // unlinked
                path: "/tmp/p2".into(),
                ..Default::default()
            },
            Project {
                project_id: "p3".into(),
                path: "".into(), // no path
                ..Default::default()
            },
        ];
        let map = proj_id_to_canonical_paths(&projects);
        assert!(map.contains_key("p1"));
        assert!(!map.contains_key(""));
        assert!(!map.contains_key("p3"));
        assert_eq!(map.len(), 1);
    }
}
