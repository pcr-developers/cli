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

pub fn get_branch(project_path: &str) -> String {
    if project_path.is_empty() {
        return String::new();
    }
    run(
        &["-C", project_path, "rev-parse", "--abbrev-ref", "HEAD"],
        None,
    )
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
    let out = run(&["-C", project_path, "status", "--porcelain"], None);
    if out.is_empty() {
        return String::new();
    }
    let mut sb = String::new();
    for line in out.split('\n') {
        if line.len() < 4 || &line[..2] != "??" {
            continue;
        }
        let rel = line[3..].trim();
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
