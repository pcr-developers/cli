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

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
    // Cache key includes cwd because the same relative path resolves
    // differently from different working directories.
    let cache_key = (trimmed.to_string(), base_cwd.unwrap_or("").to_string());
    if let Some(cached) = lookup_normalize_cache(&cache_key) {
        return cached;
    }
    let result = normalize_path_uncached(trimmed, base_cwd);
    insert_normalize_cache(cache_key, result.clone());
    result
}

fn normalize_path_uncached(trimmed: &str, base_cwd: Option<&str>) -> Option<String> {
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
    // symlinks. If the file doesn't exist (renamed/deleted/about-to-be-
    // created), walk up to the deepest existing ancestor, canonicalize
    // THAT, then re-append the missing tail. This is critical on macOS
    // where `/tmp` is itself a symlink to `/private/tmp` — without the
    // walk-up, a project canonicalizes to `/private/tmp/...` while a
    // tool-call path to a non-existent file under it stays as `/tmp/...`
    // and attribution silently fails.
    let resolved = canonicalize_or_walk_up(&abs_string);
    Some(strip_windows_verbatim_prefix(&resolved))
}

/// Try `std::fs::canonicalize` on the full path. If that fails (file
/// doesn't exist), walk up the path until we find an ancestor that
/// canonicalize accepts, then re-append the missing tail. This makes
/// symlinked-ancestor cases (like macOS's `/tmp` → `/private/tmp`)
/// work even when the leaf file isn't on disk.
///
/// Falls back to a purely textual `clean_path` only when no ancestor
/// can be canonicalized — should be vanishingly rare (effectively
/// impossible since `/` always canonicalizes).
fn canonicalize_or_walk_up(abs_string: &str) -> String {
    if let Ok(c) = std::fs::canonicalize(abs_string) {
        return c.to_string_lossy().into_owned();
    }
    let p = Path::new(abs_string);
    // Collect the trailing components that don't exist, walking up.
    let mut tail_segments: Vec<std::ffi::OsString> = Vec::new();
    let mut current = p;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            tail_segments.push(name.to_os_string());
        }
        // `parent` may be the empty path on relative inputs (defensive —
        // we only reach here for absolute strings, but be safe).
        if parent.as_os_str().is_empty() {
            break;
        }
        if let Ok(c) = std::fs::canonicalize(parent) {
            let mut out = c;
            // tail_segments was pushed leaf-first; reverse to apply
            // root-most-first.
            for seg in tail_segments.iter().rev() {
                out.push(seg);
            }
            return out.to_string_lossy().into_owned();
        }
        current = parent;
    }
    clean_path(abs_string)
}

/// Canonicalize a registered project path. Falls back to the literal path
/// on failure (network mount offline, project moved on disk, etc.) so we
/// still produce an attribution key — just one that won't catch
/// symlink-aliased tool calls until the project is reachable again.
///
/// Cached per-process; project paths basically never move under us, so
/// caching is effectively free.
pub fn canonicalize_project_path(project_path: &str) -> String {
    if project_path.is_empty() {
        return String::new();
    }
    if let Some(cached) = lookup_project_cache(project_path) {
        return cached;
    }
    let canon = std::fs::canonicalize(project_path)
        .ok()
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| project_path.to_string());
    let canon = strip_windows_verbatim_prefix(&canon);
    insert_project_cache(project_path.to_string(), canon.clone());
    canon
}

/// Strip Windows's `\\?\` extended-length-path prefix that
/// `std::fs::canonicalize` emits for paths near the 260-char `MAX_PATH`
/// limit (and increasingly often in modern Windows). A registered project
/// path lacks the prefix; canonicalize-of-the-tool-call always has it.
/// Without stripping, every Windows attribution silently fails for paths
/// canonicalize chooses to format that way.
///
/// On non-Windows this is an unconditional no-op — the prefix never
/// appears in any input.
fn strip_windows_verbatim_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        // \\?\UNC\server\share\... → \\server\share\...
        return format!(r"\\{rest}");
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // \\?\C:\foo → C:\foo
        return rest.to_string();
    }
    s.to_string()
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

// ─── Caches ────────────────────────────────────────────────────────────────
//
// Two per-process caches: `normalize_path` results (keyed on raw input +
// cwd) and project canonicalization. Both are unbounded — but the working
// set in practice is small (tens of distinct paths per session) so the
// memory cost is negligible and the syscall savings are large.

type NormCacheKey = (String, String);

fn normalize_cache() -> &'static Mutex<HashMap<NormCacheKey, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<NormCacheKey, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup_normalize_cache(key: &NormCacheKey) -> Option<Option<String>> {
    normalize_cache().lock().ok()?.get(key).cloned()
}

fn insert_normalize_cache(key: NormCacheKey, value: Option<String>) {
    if let Ok(mut guard) = normalize_cache().lock() {
        guard.insert(key, value);
    }
}

fn project_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup_project_cache(key: &str) -> Option<String> {
    project_cache().lock().ok()?.get(key).cloned()
}

fn insert_project_cache(key: String, value: String) {
    if let Ok(mut guard) = project_cache().lock() {
        guard.insert(key, value);
    }
}

/// Test-only: drop both caches so each test sees fresh syscalls.
#[cfg(test)]
pub(crate) fn clear_caches_for_tests() {
    if let Ok(mut g) = normalize_cache().lock() {
        g.clear();
    }
    if let Ok(mut g) = project_cache().lock() {
        g.clear();
    }
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
    fn windows_verbatim_prefix_is_stripped_from_drive_paths() {
        // Strict: not a Windows-only test — the helper must be platform-
        // independent so cross-compiled tests don't rot.
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\me\proj\foo.rs"),
            r"C:\Users\me\proj\foo.rs"
        );
    }

    #[test]
    fn windows_verbatim_prefix_is_stripped_from_unc_paths() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\foo.rs"),
            r"\\server\share\foo.rs"
        );
    }

    #[test]
    fn paths_without_verbatim_prefix_pass_through_unchanged() {
        assert_eq!(
            strip_windows_verbatim_prefix("/usr/local/bin"),
            "/usr/local/bin"
        );
        assert_eq!(strip_windows_verbatim_prefix(r"C:\foo"), r"C:\foo");
        assert_eq!(strip_windows_verbatim_prefix(""), "");
    }

    #[test]
    fn normalize_path_cache_is_idempotent() {
        clear_caches_for_tests();
        // Same input twice must give the same output. The cache is the
        // optimization; we test the contract.
        let a = normalize_path("/tmp/cached_test_path", None);
        let b = normalize_path("/tmp/cached_test_path", None);
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_path_cache_distinguishes_cwds() {
        clear_caches_for_tests();
        // The same relative path under different cwds must resolve to
        // different absolute paths — the cache key includes cwd.
        let a = normalize_path("./foo.rs", Some("/tmp/proj_a")).unwrap();
        let b = normalize_path("./foo.rs", Some("/tmp/proj_b")).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_path_caches_negative_results() {
        clear_caches_for_tests();
        // Empty input → None. Cached as None. Same input again → still None
        // and we want the same result without another fall-through cost.
        assert_eq!(normalize_path("", None), None);
        assert_eq!(normalize_path("", None), None);
    }

    #[test]
    fn canonicalize_project_path_cache_returns_consistent_results() {
        clear_caches_for_tests();
        let p = "/tmp/canonicalize_project_test";
        let a = canonicalize_project_path(p);
        let b = canonicalize_project_path(p);
        assert_eq!(a, b);
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
