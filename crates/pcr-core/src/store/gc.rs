//! Garbage collection of old commits. Mirrors `cli/internal/store/gc.go`.

use anyhow::Result;
use chrono::{Duration, Utc};
use rusqlite::params;
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::store::db::open;

fn collect_strings(sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
    let conn = open();
    let mut stmt = conn.prepare(sql)?;
    let out: Vec<String> = stmt
        .query_map(params, |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(out)
}

pub fn gc_pushed(older_than_days: i64) -> Result<i64> {
    let cutoff = (Utc::now() - Duration::days(older_than_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let ids = collect_strings(
        "SELECT id FROM prompt_commits WHERE pushed_at IS NOT NULL AND pushed_at < ?",
        &[&cutoff as &dyn rusqlite::ToSql],
    )?;
    delete_commits(&ids)
}

pub fn gc_all_pushed() -> Result<i64> {
    let ids = collect_strings(
        "SELECT id FROM prompt_commits WHERE pushed_at IS NOT NULL",
        &[],
    )?;
    delete_commits(&ids)
}

pub fn gc_unpushed() -> Result<i64> {
    let ids = collect_strings("SELECT id FROM prompt_commits WHERE pushed_at IS NULL", &[])?;
    delete_commits(&ids)
}

/// Delete every unbundled draft (status = 'draft') across all projects.
/// Returns the number of rows removed. Bundled / pushed prompts are
/// untouched — they only disappear via `gc_unpushed` or `gc_pushed`.
pub fn gc_drafts() -> Result<i64> {
    let conn = open();
    let n = conn.execute("DELETE FROM drafts WHERE status = 'draft'", [])?;
    Ok(n as i64)
}

/// Delete unbundled drafts older than `older_than_days`. Useful when a
/// draft list has accumulated stale prompts from old experiments and
/// the user wants to reclaim the view without losing recent work.
pub fn gc_drafts_older_than(older_than_days: i64) -> Result<i64> {
    let cutoff = (Utc::now() - Duration::days(older_than_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let conn = open();
    let n = conn.execute(
        "DELETE FROM drafts WHERE status = 'draft' AND captured_at < ?",
        params![cutoff],
    )?;
    Ok(n as i64)
}

pub fn gc_orphaned(project_path: &Path) -> Result<i64> {
    let rows: Vec<(String, String)> = {
        let conn = open();
        let mut stmt =
            conn.prepare("SELECT id, head_sha FROM prompt_commits WHERE pushed_at IS NULL")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    if rows.is_empty() {
        return Ok(0);
    }
    // The previous shape ran one `git cat-file -e <sha>` per commit
    // — O(N) git processes per GC pass. Replaced with a single
    // `git cat-file --batch-check` invocation that reads SHAs on
    // stdin and emits one line per query. (The audit suggested
    // `git for-each-ref refs/heads/`, but that's the wrong
    // primitive for "does this exact SHA still exist": branches
    // walk the reflog forward, not from the commit graph
    // backward. `--batch-check` is git's purpose-built answer for
    // this exact query.) Keeps the orphan-detection semantics
    // byte-identical: a SHA is orphaned iff `git cat-file` can't
    // resolve it (rebased away, branch deleted, repo re-cloned).
    let missing_shas = shas_missing_from_repo(project_path, rows.iter().map(|(_, s)| s.as_str()));
    let orphans: Vec<String> = rows
        .into_iter()
        .filter(|(_, sha)| missing_shas.contains(sha))
        .map(|(id, _)| id)
        .collect();
    if orphans.is_empty() {
        return Ok(0);
    }

    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for id in &orphans {
        let draft_ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?")?;
            let ids: Vec<String> = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };
        tx.execute(
            "DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?",
            params![id],
        )?;
        for did in draft_ids {
            tx.execute(
                "UPDATE drafts SET status = 'draft' WHERE id = ? AND status = 'committed'",
                params![did],
            )?;
        }
        tx.execute("DELETE FROM prompt_commits WHERE id = ?", params![id])?;
    }
    tx.commit()?;
    Ok(orphans.len() as i64)
}

/// Ask git which of `shas` are NOT present as commit objects in
/// `repo`. Single subprocess regardless of input size — `git
/// cat-file --batch-check` reads SHAs on stdin (one per line) and
/// emits one line per query. We treat any non-resolvable line as
/// "missing" (matches the original per-SHA `cat-file -e` exit
/// status: success ⇒ present, failure ⇒ absent).
///
/// If git fails to start at all (no git on PATH, repo gone), we
/// return an empty set — that means "nothing is orphaned by
/// git", i.e. we'd rather skip a GC cycle than nuke unpushed
/// commits because the user's git binary is mis-installed.
fn shas_missing_from_repo<'a>(
    repo: &Path,
    shas: impl IntoIterator<Item = &'a str>,
) -> HashSet<String> {
    let shas: Vec<String> = shas.into_iter().map(String::from).collect();
    if shas.is_empty() {
        return HashSet::new();
    }
    let mut child = match Command::new("git")
        .arg("cat-file")
        .arg("--batch-check=%(objectname) %(objecttype)")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .current_dir(repo)
        .spawn()
    {
        Ok(c) => c,
        // git unavailable / repo broken — preserve unpushed work
        // rather than wrongly classifying everything as orphaned.
        Err(_) => return HashSet::new(),
    };

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort: if writes start failing partway through,
        // we still wait_with_output so we don't leak the child.
        // The stdout we did receive will be parsed below; any
        // unwritten SHAs are simply omitted from the missing set
        // (treated as present), again erring on the side of NOT
        // deleting unpushed history we're unsure about.
        for sha in &shas {
            if writeln!(stdin, "{sha}").is_err() {
                break;
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return HashSet::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut missing: HashSet<String> = HashSet::new();
    for line in stdout.lines() {
        // `--batch-check` emits `<sha> missing` for unknown
        // objects and `<sha> <type> <size>` for resolvable ones.
        // We only care about the "missing" sentinel.
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        if parts.next() == Some("missing") {
            missing.insert(name.to_string());
        }
    }
    missing
}

fn delete_commits(ids: &[String]) -> Result<i64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut deleted: i64 = 0;
    let conn = open();
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        let n: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM prompt_commit_items WHERE prompt_commit_id = ?",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        deleted += n;

        let draft_ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT draft_id FROM prompt_commit_items WHERE prompt_commit_id = ?")?;
            let ids: Vec<String> = stmt
                .query_map(params![id], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };
        tx.execute(
            "DELETE FROM prompt_commit_items WHERE prompt_commit_id = ?",
            params![id],
        )?;
        for did in draft_ids {
            tx.execute("DELETE FROM drafts WHERE id = ?", params![did])?;
        }
        tx.execute("DELETE FROM prompt_commits WHERE id = ?", params![id])?;
    }
    tx.commit()?;
    Ok(deleted)
}

/// Candidate scoring — draft relevance to a list of changed files. Mirrors
/// `cli/internal/store/drafts.go::GetCandidatesForCommit`. Placed here to
/// keep the drafts module from sprawling; it consumes only public drafts API.
pub fn get_candidates_for_commit(
    project_ids: &[String],
    project_names: &[String],
    changed_files: &[String],
) -> Result<(
    Vec<crate::store::DraftRecord>,
    Vec<crate::store::DraftRecord>,
)> {
    use crate::store::{get_drafts_by_status, DraftStatus};
    let drafts = get_drafts_by_status(DraftStatus::Draft, project_ids, project_names)?;
    if changed_files.is_empty() {
        return Ok((Vec::new(), drafts));
    }
    let mut scored: Vec<(crate::store::DraftRecord, i64)> = Vec::with_capacity(drafts.len());
    for d in drafts {
        let mut match_count: i64 = 0;
        let mut touched: std::collections::HashSet<String> = Default::default();
        for tc in &d.tool_calls {
            let mut path = String::new();
            if let Some(input) = tc.get("input").and_then(|v| v.as_object()) {
                path = input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .or_else(|| input.get("file_path").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
            }
            if path.is_empty() {
                if let Some(p) = tc.get("path").and_then(|v| v.as_str()) {
                    path = p.to_string();
                }
            }
            if path.is_empty() {
                continue;
            }
            for cf in changed_files {
                if touched.contains(cf) {
                    continue;
                }
                if path.ends_with(cf.as_str())
                    || cf.ends_with(path.as_str())
                    || path.contains(cf.as_str())
                    || cf.contains(path.as_str())
                {
                    match_count += 1;
                    touched.insert(cf.clone());
                }
            }
        }

        let text = format!("{} {}", d.prompt_text, d.response_text);
        for cf in changed_files {
            if touched.contains(cf) {
                continue;
            }
            let base = std::path::Path::new(cf)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(cf);
            let stem = std::path::Path::new(cf)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(base);
            if text.contains(cf) || text.contains(base) || (stem.len() >= 5 && text.contains(stem))
            {
                match_count += 1;
                touched.insert(cf.clone());
            }
        }
        scored.push((d, match_count));
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    let mut relevant: Vec<crate::store::DraftRecord> = Vec::new();
    let mut unrelated: Vec<crate::store::DraftRecord> = Vec::new();
    for (d, n) in scored {
        if n > 0 {
            relevant.push(d);
        } else {
            unrelated.push(d);
        }
    }
    Ok((relevant, unrelated))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: initialise a tiny git repo with one commit, return
    /// the temp dir (held for lifetime) and the commit's SHA so
    /// tests can verify the existence-check semantics.
    fn init_repo_with_one_commit() -> Option<(tempfile::TempDir, String)> {
        // git might not be on PATH in some sandboxed CI envs.
        // Probe once and bail out (skipping the test) if so —
        // we don't want a missing git binary to flake the suite.
        if Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            return None;
        }

        let tmp = tempfile::TempDir::new().ok()?;
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(tmp.path())
                .env("GIT_AUTHOR_NAME", "pcr-test")
                .env("GIT_AUTHOR_EMAIL", "pcr-test@example.invalid")
                .env("GIT_COMMITTER_NAME", "pcr-test")
                .env("GIT_COMMITTER_EMAIL", "pcr-test@example.invalid")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()
                .filter(|s| s.success())
        };
        run(&["init", "--initial-branch=main"]).or_else(|| run(&["init"]))?;
        std::fs::write(tmp.path().join("README"), b"hello\n").ok()?;
        run(&["add", "."])?;
        run(&["commit", "-m", "first"])?;

        let sha = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .ok()?;
        if !sha.status.success() {
            return None;
        }
        let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();
        if sha.is_empty() {
            return None;
        }
        Some((tmp, sha))
    }

    #[test]
    fn batch_check_marks_only_unknown_shas_as_missing() {
        let Some((repo, head_sha)) = init_repo_with_one_commit() else {
            eprintln!(
                "skipping batch-check test: git unavailable or unable to init a fixture repo"
            );
            return;
        };
        // 40 zeros: well-formed SHA1, but cannot resolve to any
        // commit object in this fresh repo.
        let bogus_sha = "0".repeat(40);

        let missing = shas_missing_from_repo(repo.path(), [head_sha.as_str(), bogus_sha.as_str()]);

        assert!(
            !missing.contains(&head_sha),
            "live HEAD sha {head_sha} must NOT be reported missing"
        );
        assert!(
            missing.contains(&bogus_sha),
            "unknown sha {bogus_sha} must be reported missing"
        );
        assert_eq!(
            missing.len(),
            1,
            "exactly one of the two SHAs is unknown; got {missing:?}"
        );
    }

    #[test]
    fn batch_check_returns_empty_when_input_is_empty() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // Doesn't even need to be a git repo — empty input
        // short-circuits before spawning git.
        let missing = shas_missing_from_repo(tmp.path(), std::iter::empty::<&str>());
        assert!(missing.is_empty());
    }

    #[test]
    fn batch_check_returns_empty_on_git_failure() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // Not a git repo at all. `git cat-file --batch-check`
        // will fail to start (or exit non-zero immediately).
        // We expect an empty `missing` set — the conservative
        // choice that protects unpushed work from being GC'd
        // because git happens to be broken / mis-configured on
        // this machine.
        let missing = shas_missing_from_repo(tmp.path(), ["deadbeef".repeat(5).as_str()]);
        assert!(
            missing.is_empty(),
            "git failure must NOT classify SHAs as missing (would wrongly GC unpushed work); \
             got {missing:?}"
        );
    }
}
