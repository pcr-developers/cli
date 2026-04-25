//! Git wrappers. Direct port of `cli/internal/sources/shared/git.go`.
//!
//! Each function shells out to the `git` binary exactly like the Go version.
//! On Windows, `git.exe` lives in `C:\Program Files\Git\` which is an
//! AppLocker-allowed path, so spawning it from inside `node.exe` (for the
//! napi build) or from a standalone binary (for the brew build) always
//! passes AppLocker path rules.

use std::path::Path;
use std::process::Command;

pub fn get_head_sha(project_path: &str) -> String {
    if project_path.is_empty() {
        return String::new();
    }
    run(&["-C", project_path, "rev-parse", "HEAD"], None)
}

/// Returns the current branch name for `project_path`, or empty string
/// when the working tree is in a detached-HEAD state.
///
/// `git rev-parse --abbrev-ref HEAD` returns the literal string "HEAD"
/// when detached, which downstream display / push code would mistake for
/// a real branch named "HEAD". Filter it here once instead of at every
/// call site.
pub fn get_branch(project_path: &str) -> String {
    if project_path.is_empty() {
        return String::new();
    }
    let raw = run(
        &["-C", project_path, "rev-parse", "--abbrev-ref", "HEAD"],
        None,
    );
    if raw == "HEAD" {
        String::new()
    } else {
        raw
    }
}

/// Returns true when `project_path` is inside a git working tree. Used by
/// the watchers to distinguish "no diff because there's nothing to diff"
/// from "no diff because git isn't available". The latter ends up tagged
/// in `file_context.git_unavailable: true` so reviewers see why the diff
/// is empty instead of being misled.
///
/// Empty paths return false (vacuous — not even attempted). A successful
/// `git rev-parse --is-inside-work-tree` writes "true" to stdout.
///
/// Result is cached per-process — projects don't move between
/// "is a git repo" and "isn't a git repo" within a watcher's lifetime,
/// and the watchers call this on every save.
pub fn is_git_repo(project_path: &str) -> bool {
    if project_path.is_empty() {
        return false;
    }
    if let Some(cached) = lookup_is_git_repo_cache(project_path) {
        return cached;
    }
    let out = run(
        &["-C", project_path, "rev-parse", "--is-inside-work-tree"],
        None,
    );
    let result = out.trim() == "true";
    insert_is_git_repo_cache(project_path.to_string(), result);
    result
}

fn is_git_repo_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, bool>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn lookup_is_git_repo_cache(key: &str) -> Option<bool> {
    is_git_repo_cache().lock().ok()?.get(key).copied()
}

fn insert_is_git_repo_cache(key: String, value: bool) {
    if let Ok(mut g) = is_git_repo_cache().lock() {
        g.insert(key, value);
    }
}

/// `git diff HEAD` combined with a synthetic diff of untracked files.
/// Capped at 50 KB. Matches `shared.GetGitDiff`.
pub fn get_git_diff(project_path: &str) -> String {
    if project_path.is_empty() {
        return String::new();
    }
    let tracked = run(&["diff", "HEAD"], Some(project_path));
    let untracked = untracked_diff(project_path);
    let combined = format!("{tracked}{untracked}");
    if combined.is_empty() {
        return String::new();
    }
    const MAX: usize = 50_000;
    if combined.len() > MAX {
        let mut out = combined[..MAX].to_string();
        out.push_str("\n[truncated]");
        out
    } else {
        combined
    }
}

fn untracked_diff(project_path: &str) -> String {
    // NUL-terminated porcelain so filenames with spaces, quotes, or
    // newlines round-trip verbatim. Without `-z`, git shell-escapes
    // problem paths and a naive `line[3..].trim()` parser would
    // produce literal `"foo bar.rs"` strings that fail every later
    // file read.
    let cmd = std::process::Command::new("git")
        .args(["-C", project_path, "status", "--porcelain=v1", "-z"])
        .output();
    let Ok(output) = cmd else {
        return String::new();
    };
    if !output.status.success() || output.stdout.is_empty() {
        return String::new();
    }
    let mut sb = String::new();
    let bytes = &output.stdout;
    for field in bytes.split(|b| *b == 0) {
        if field.len() < 4 || &field[..2] != b"??" {
            continue;
        }
        let Ok(rel) = std::str::from_utf8(&field[3..]) else {
            continue;
        };
        if rel.is_empty() || rel.ends_with('/') {
            continue;
        }
        let Ok(content) = std::fs::read(Path::new(project_path).join(rel)) else {
            continue;
        };
        let check_len = content.len().min(8192);
        if content[..check_len].contains(&0) {
            continue;
        }
        let s = String::from_utf8_lossy(&content);
        let trimmed = s.trim_end_matches('\n');
        let lines: Vec<&str> = trimmed.split('\n').collect();
        sb.push_str(&format!(
            "diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n@@ -0,0 +1,{} @@\n",
            lines.len()
        ));
        for l in lines {
            sb.push_str(&format!("+{l}\n"));
        }
    }
    sb
}

/// `git log --format=%H --after=<sinceISO>` from inside `project_path`.
pub fn get_commits_since(project_path: &str, since_iso: &str) -> Vec<String> {
    let out = run(
        &["log", "--format=%H", &format!("--after={since_iso}")],
        Some(project_path),
    );
    filter_non_empty(out.trim().split('\n'))
}

/// `git log --format=%H --no-merges` with optional millisecond epoch bounds.
pub fn get_commit_range(
    project_path: &str,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
) -> Vec<String> {
    let mut args: Vec<String> = vec!["log".into(), "--format=%H".into(), "--no-merges".into()];
    if let Some(ms) = since_ms {
        args.push(format!("--after=@{}", ms / 1000));
    }
    if let Some(ms) = until_ms {
        args.push(format!("--before=@{}", ms / 1000));
    }
    let out = run_owned(&args, Some(project_path));
    filter_non_empty(out.trim().split('\n'))
}

pub fn filter_non_empty<'a, I: IntoIterator<Item = &'a str>>(it: I) -> Vec<String> {
    it.into_iter()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn run(args: &[&str], cwd: Option<&str>) -> String {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
        _ => String::new(),
    }
}

fn run_owned(args: &[String], cwd: Option<&str>) -> String {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
        _ => String::new(),
    }
}

/// Wrapper to run a git subcommand and return trimmed stdout. Public for
/// commands that need their own git calls (push, init, log, etc.).
pub fn git_output(args: &[&str]) -> String {
    run(args, None)
}

pub fn git_output_in(dir: &str, args: &[&str]) -> String {
    run(args, Some(dir))
}
