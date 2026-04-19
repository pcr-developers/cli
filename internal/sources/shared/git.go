package shared

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// GetHeadSha returns the current HEAD SHA for the given project path.
func GetHeadSha(projectPath string) string {
	if projectPath == "" {
		return ""
	}
	out, err := exec.Command("git", "-C", projectPath, "rev-parse", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// GetBranch returns the current git branch name for the given project path.
func GetBranch(projectPath string) string {
	if projectPath == "" {
		return ""
	}
	out, err := exec.Command("git", "-C", projectPath, "rev-parse", "--abbrev-ref", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// GetGitDiff returns the combined diff of tracked changes and new untracked
// files for the given project path. Output is truncated to 50KB.
func GetGitDiff(projectPath string) string {
	if projectPath == "" {
		return ""
	}
	cmd := exec.Command("git", "diff", "HEAD")
	cmd.Dir = projectPath
	tracked, _ := cmd.Output()

	untracked := untrackedDiff(projectPath)

	combined := string(tracked) + untracked
	if combined == "" {
		return ""
	}
	const maxBytes = 50_000
	if len(combined) > maxBytes {
		return combined[:maxBytes] + "\n[truncated]"
	}
	return combined
}

// untrackedDiff generates a unified-diff representation of untracked new files.
func untrackedDiff(projectPath string) string {
	out, err := exec.Command("git", "-C", projectPath, "status", "--porcelain").Output()
	if err != nil || len(out) == 0 {
		return ""
	}
	var sb strings.Builder
	for _, line := range strings.Split(string(out), "\n") {
		if len(line) < 4 || line[:2] != "??" {
			continue
		}
		rel := strings.TrimSpace(line[3:])
		if rel == "" || strings.HasSuffix(rel, "/") {
			continue
		}
		content, err := os.ReadFile(filepath.Join(projectPath, rel))
		if err != nil {
			continue
		}
		check := content
		if len(check) > 8192 {
			check = check[:8192]
		}
		if strings.ContainsRune(string(check), 0) {
			continue
		}
		lines := strings.Split(strings.TrimRight(string(content), "\n"), "\n")
		fmt.Fprintf(&sb, "diff --git a/%s b/%s\nnew file mode 100644\n--- /dev/null\n+++ b/%s\n@@ -0,0 +1,%d @@\n",
			rel, rel, rel, len(lines))
		for _, l := range lines {
			fmt.Fprintf(&sb, "+%s\n", l)
		}
	}
	return sb.String()
}

// GetCommitsSince returns commit SHAs after the given ISO timestamp.
func GetCommitsSince(projectPath, sinceISO string) []string {
	cmd := exec.Command("git", "log", "--format=%H", "--after="+sinceISO)
	cmd.Dir = projectPath
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	return FilterNonEmpty(strings.Split(strings.TrimSpace(string(out)), "\n"))
}

// GetCommitRange returns commit SHAs between two unix-millisecond timestamps.
func GetCommitRange(projectPath string, sinceMs, untilMs *int64) []string {
	args := []string{"log", "--format=%H", "--no-merges"}
	if sinceMs != nil {
		t := fmt.Sprintf("%d", *sinceMs/1000)
		args = append(args, "--after=@"+t)
	}
	if untilMs != nil {
		t := fmt.Sprintf("%d", *untilMs/1000)
		args = append(args, "--before=@"+t)
	}
	cmd := exec.Command("git", args...)
	cmd.Dir = projectPath
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	return FilterNonEmpty(strings.Split(strings.TrimSpace(string(out)), "\n"))
}

// FilterNonEmpty removes empty/whitespace-only strings from a slice.
func FilterNonEmpty(lines []string) []string {
	var result []string
	for _, l := range lines {
		if strings.TrimSpace(l) != "" {
			result = append(result, l)
		}
	}
	return result
}

// TouchedProjectIDs returns all registered project IDs whose files appear in
// tool call paths.
func TouchedProjectIDs(toolCalls []map[string]any, projByID map[string]string) []string {
	seen := map[string]bool{}
	for _, tc := range toolCalls {
		path := extractPathFromToolCall(tc)
		if path == "" {
			continue
		}
		for id, projPath := range projByID {
			if projPath != "" && strings.HasPrefix(path, projPath+"/") {
				seen[id] = true
			}
		}
	}
	ids := make([]string, 0, len(seen))
	for id := range seen {
		ids = append(ids, id)
	}
	return ids
}

// RepoSnapshots returns git snapshots for repos OTHER than primaryProjectID
// that are referenced by tool call file paths.
func RepoSnapshots(toolCalls []map[string]any, primaryProjectID string, projByID map[string]string) map[string]any {
	result := map[string]any{}
	for _, tc := range toolCalls {
		path := extractPathFromToolCall(tc)
		if path == "" {
			continue
		}
		for id, projPath := range projByID {
			if id == primaryProjectID || projPath == "" {
				continue
			}
			if strings.HasPrefix(path, projPath+"/") {
				if _, ok := result[id]; !ok {
					result[id] = map[string]any{
						"head_sha": GetHeadSha(projPath),
						"git_diff": GetGitDiff(projPath),
					}
				}
			}
		}
	}
	if len(result) == 0 {
		return nil
	}
	return result
}

// ChangedFilesFromToolCalls extracts file paths from write-oriented tool calls.
func ChangedFilesFromToolCalls(toolCalls []map[string]any) []string {
	writeTools := map[string]bool{
		"write_file":              true,
		"create_file":             true,
		"edit_file":               true,
		"replace_string_in_file":  true,
		"multi_replace_string_in_file": true,
		"edit_notebook_file":      true,
		"Write":                   true, // Claude Code tool name
	}

	seen := map[string]bool{}
	var files []string
	for _, tc := range toolCalls {
		tool, _ := tc["tool"].(string)
		if !writeTools[tool] {
			continue
		}
		path := extractPathFromToolCall(tc)
		if path != "" && !seen[path] {
			seen[path] = true
			files = append(files, path)
		}
	}
	return files
}

// extractPathFromToolCall extracts a file path from a tool call's input.
func extractPathFromToolCall(tc map[string]any) string {
	if input, ok := tc["input"].(map[string]any); ok {
		for _, key := range []string{"path", "file_path", "filePath"} {
			if p, ok := input[key].(string); ok && p != "" {
				return p
			}
		}
	}
	// Fallback: top-level path
	if p, ok := tc["path"].(string); ok && p != "" {
		return p
	}
	return ""
}
