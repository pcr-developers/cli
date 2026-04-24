//! Text / selection helpers mirroring `cmd/helpers.go`.

use std::collections::HashSet;

/// Collapses runs of whitespace into single spaces and truncates to `n`
/// characters, appending a `…` if the flat form exceeds `n`. Mirrors
/// `cmd/helpers.go::truncate`.
pub fn truncate(text: &str, n: usize) -> String {
    let flat: Vec<&str> = text.split_whitespace().collect();
    let flat = flat.join(" ");
    if flat.chars().count() > n {
        let take = n.saturating_sub(1);
        let trunc: String = flat.chars().take(take).collect();
        format!("{trunc}…")
    } else {
        flat
    }
}

/// Returns a useful preview of a multi-line prompt for list views. Matches
/// `cmd/helpers.go::promptPreview`.
pub fn prompt_preview(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim().split('\n').collect();
    let mut meaningful: Vec<&str> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("@/") || trimmed.starts_with("@~") {
            continue;
        }
        if trimmed.starts_with("/Users/") || trimmed.starts_with("/home/") {
            continue;
        }
        meaningful.push(trimmed);
    }
    if meaningful.is_empty() {
        return truncate(text, n);
    }
    let mut preview: &str = meaningful.last().copied().unwrap();
    if meaningful.len() > 1 && meaningful[0].len() <= n / 2 {
        preview = meaningful[0];
    }
    truncate(preview, n)
}

/// `s == 1 ? "" : "s"`. Matches `cmd/helpers.go::plural`.
pub fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Parse a selection like `1-5`, `1,3,7`, or a single number against a bounded
/// range `[0, max)` and return the 0-based indices in first-seen order.
/// Matches `cmd/helpers.go::parseSelectionIndices`.
pub fn parse_selection_indices(input: &str, max: usize) -> Vec<usize> {
    let mut seen: HashSet<usize> = HashSet::new();
    let mut result: Vec<usize> = Vec::new();

    for part in input.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(hyphen) = t.find('-') {
            if hyphen == 0 {
                continue; // e.g. "-3" isn't a valid range
            }
            let left = t[..hyphen].trim();
            let right = t[hyphen + 1..].trim();
            let from: Result<usize, _> = left.parse();
            let to: Result<usize, _> = right.parse();
            let (Ok(from), Ok(to)) = (from, to) else {
                eprintln!(
                    "PCR: Invalid selection {t:?} — use numbers only (e.g. 1-4, 2,5,7, or all)"
                );
                continue;
            };
            if from > to {
                eprintln!(
                    "PCR: Invalid range {t:?} — use low-to-high order (e.g. {to}-{from} not {from}-{to})"
                );
                continue;
            }
            for n in from..=to {
                if n == 0 {
                    continue;
                }
                let i = n - 1;
                if i < max && seen.insert(i) {
                    result.push(i);
                }
            }
        } else if let Ok(n) = t.parse::<usize>() {
            if n == 0 {
                continue;
            }
            let i = n - 1;
            if i < max && seen.insert(i) {
                result.push(i);
            }
        }
    }
    result
}

/// Parse the first positive index from a space/comma-separated list against
/// `[1, max]`. Matches `cmd/helpers.go::parseFirstIndex`.
pub fn parse_first_index(resp: &str, max: usize) -> Option<usize> {
    let first = resp
        .split(|c: char| c == ',' || c == ' ')
        .find(|f| !f.trim().is_empty())?
        .trim();
    let n: usize = first.parse().ok()?;
    if n >= 1 && n <= max {
        Some(n - 1)
    } else {
        None
    }
}

/// Coerce a JSON-decoded numeric value to `f64`. Matches `cmd/helpers.go::toFloat64`.
pub fn to_f64(v: &serde_json::Value) -> f64 {
    match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_collapses_whitespace_and_appends_ellipsis() {
        assert_eq!(truncate("hello   world", 20), "hello world");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn prompt_preview_drops_at_file_lines() {
        let text = "@/foo/bar.rs\n\nfix the bug in main()";
        assert_eq!(prompt_preview(text, 40), "fix the bug in main()");
    }

    #[test]
    fn parse_selection_ranges_and_dedup() {
        assert_eq!(parse_selection_indices("1-3,2,5", 10), vec![0, 1, 2, 4]);
        assert_eq!(parse_selection_indices("11-12", 5), Vec::<usize>::new());
        assert_eq!(parse_selection_indices("0,1", 5), vec![0]);
    }

    #[test]
    fn parse_first_index_bounds() {
        assert_eq!(parse_first_index("3, 4", 5), Some(2));
        assert_eq!(parse_first_index("9", 5), None);
        assert_eq!(parse_first_index("", 5), None);
    }

    #[test]
    fn plural_english() {
        assert_eq!(plural(1), "");
        assert_eq!(plural(0), "s");
        assert_eq!(plural(2), "s");
    }
}
