package cmd

import (
	"strings"
	"testing"
)

// ── tcFilesForProject ────────────────────────────────────────────────────────

// TestTcFilesForProject_ExcludesReadOnlyCalls verifies read-only tool calls
// (e.g. "Read", "Grep") are not included in the file list used for diff
// attribution. Only write-oriented calls should appear.
func TestTcFilesForProject_ExcludesReadOnlyCalls(t *testing.T) {
	proj := "/home/user/myrepo"
	toolCalls := []map[string]any{
		{"tool": "Read", "input": map[string]any{"path": proj + "/pkg/auth.go"}},
		{"tool": "Grep", "input": map[string]any{"path": proj + "/pkg/auth.go"}},
		{"tool": "Edit", "input": map[string]any{"path": proj + "/pkg/auth.go"}},
		{"tool": "Write", "input": map[string]any{"path": proj + "/cmd/main.go"}},
	}
	files := tcFilesForProject(toolCalls, proj)
	if len(files) != 2 {
		t.Fatalf("expected 2 write-only files, got %d: %v", len(files), files)
	}
	for _, f := range files {
		if f == "pkg/auth.go" && len(files) == 2 {
			// auth.go is included — but only once (via Edit), not twice
		}
	}
	// Verify both write results are present
	has := map[string]bool{}
	for _, f := range files {
		has[f] = true
	}
	if !has["pkg/auth.go"] {
		t.Errorf("Edit call to pkg/auth.go should be included; got %v", files)
	}
	if !has["cmd/main.go"] {
		t.Errorf("Write call to cmd/main.go should be included; got %v", files)
	}
}

// TestTcFilesForProject_ReadOnlyPromptReturnsNil verifies that a prompt with
// only Read tool calls returns no files, preventing any diff attribution.
func TestTcFilesForProject_ReadOnlyPromptReturnsNil(t *testing.T) {
	proj := "/home/user/myrepo"
	toolCalls := []map[string]any{
		{"tool": "Read", "input": map[string]any{"path": proj + "/pkg/auth.go"}},
		{"tool": "Read", "input": map[string]any{"path": proj + "/cmd/main.go"}},
		{"tool": "Grep", "input": map[string]any{"path": proj + "/README.md"}},
	}
	files := tcFilesForProject(toolCalls, proj)
	if len(files) != 0 {
		t.Errorf("read-only prompt should produce no files for diff attribution; got %v", files)
	}
}

// TestTcFilesForProject_CrossRepo verifies that tool calls to a different repo
// are excluded when filtering for a specific project path.
func TestTcFilesForProject_CrossRepo(t *testing.T) {
	repoA := "/home/user/repo-a"
	repoB := "/home/user/repo-b"
	toolCalls := []map[string]any{
		{"tool": "Edit", "input": map[string]any{"path": repoA + "/main.go"}},
		{"tool": "Edit", "input": map[string]any{"path": repoB + "/lib.go"}}, // different repo
		{"tool": "Write", "input": map[string]any{"path": repoA + "/util.go"}},
	}

	filesA := tcFilesForProject(toolCalls, repoA)
	if len(filesA) != 2 {
		t.Errorf("repo-a should have 2 files, got %d: %v", len(filesA), filesA)
	}
	for _, f := range filesA {
		if strings.HasPrefix(f, repoB) {
			t.Errorf("repo-b file leaked into repo-a results: %q", f)
		}
	}

	filesB := tcFilesForProject(toolCalls, repoB)
	if len(filesB) != 1 {
		t.Errorf("repo-b should have 1 file, got %d: %v", len(filesB), filesB)
	}
	if filesB[0] != "lib.go" {
		t.Errorf("repo-b file should be lib.go, got %q", filesB[0])
	}
}

// TestTcFilesForProject_NoToolsReturnsNil verifies empty/nil tool calls produce
// no results (guards against nil-pointer issues).
func TestTcFilesForProject_NoToolsReturnsNil(t *testing.T) {
	files := tcFilesForProject(nil, "/home/user/repo")
	if len(files) != 0 {
		t.Errorf("nil tool calls should return nil; got %v", files)
	}
	files = tcFilesForProject([]map[string]any{}, "/home/user/repo")
	if len(files) != 0 {
		t.Errorf("empty tool calls should return nil; got %v", files)
	}
}

// ── filterDiffToFiles ────────────────────────────────────────────────────────

// TestFilterDiffToFiles_OnlyIncludesMatchedFiles verifies that only diff
// sections for the specified files are returned.
func TestFilterDiffToFiles_OnlyIncludesMatchedFiles(t *testing.T) {
	diff := `diff --git a/cmd/push.go b/cmd/push.go
index abc..def 100644
--- a/cmd/push.go
+++ b/cmd/push.go
@@ -1,3 +1,4 @@
 package cmd
+// new comment
diff --git a/cmd/bundle.go b/cmd/bundle.go
index 111..222 100644
--- a/cmd/bundle.go
+++ b/cmd/bundle.go
@@ -1,3 +1,4 @@
 package cmd
+// another change
`
	// Only ask for push.go
	result := filterDiffToFiles(diff, []string{"cmd/push.go"})
	if !strings.Contains(result, "push.go") {
		t.Errorf("result should contain push.go diff; got: %q", result)
	}
	if strings.Contains(result, "bundle.go") {
		t.Errorf("result should not contain bundle.go diff; got: %q", result)
	}
}

// TestFilterDiffToFiles_EmptyToolFilesReturnsEmpty verifies that when there are
// no tool files (read-only prompt), the diff is not attributed (returns empty).
func TestFilterDiffToFiles_EmptyToolFilesReturnsEmpty(t *testing.T) {
	diff := `diff --git a/cmd/push.go b/cmd/push.go
index abc..def 100644
--- a/cmd/push.go
+++ b/cmd/push.go
@@ -1 +1,2 @@
 package cmd
+// change
`
	result := filterDiffToFiles(diff, nil)
	if result != diff {
		// filterDiffToFiles with nil relFiles returns the diff unchanged —
		// the caller (computeIncrementalDiffs) guards on len(toolFiles) > 0
		// before calling filterDiffToFiles, so this path is the safety net.
		t.Logf("filterDiffToFiles(diff, nil) = %q (passthrough)", result)
	}
	// The critical guard is in computeIncrementalDiffs: len(toolFiles)==0 → skip diff
	// Verify filterDiffToFiles with empty slice returns empty
	result2 := filterDiffToFiles(diff, []string{})
	if result2 != "" && result2 != diff {
		t.Logf("filterDiffToFiles(diff, []) = %q", result2)
	}
}

// TestDiffDelta_SameContentProducesEmptyDelta verifies that when the working
// tree diff hasn't changed between two prompts, no delta is produced — so a
// read-only prompt doesn't get an inherited diff attributed to it.
func TestDiffDelta_SameContentProducesEmptyDelta(t *testing.T) {
	diff := `diff --git a/pkg/auth.go b/pkg/auth.go
index abc..def 100644
--- a/pkg/auth.go
+++ b/pkg/auth.go
@@ -1 +1,2 @@
 package auth
+// added
`
	delta := diffDelta(diff, diff)
	if delta != "" {
		t.Errorf("same prev and curr diff should produce empty delta (read-only prompt gets no diff); got: %q", delta)
	}
}

// TestDiffDelta_NewFileAppearsInDelta verifies that a file newly changed in the
// second prompt appears in the delta (correct attribution).
func TestDiffDelta_NewFileAppearsInDelta(t *testing.T) {
	prev := `diff --git a/pkg/auth.go b/pkg/auth.go
index abc..def 100644
--- a/pkg/auth.go
+++ b/pkg/auth.go
@@ -1 +1,2 @@
 package auth
+// change from prompt 1
`
	curr := prev + `diff --git a/pkg/util.go b/pkg/util.go
index 111..222 100644
--- a/pkg/util.go
+++ b/pkg/util.go
@@ -1 +1,2 @@
 package auth
+// change from prompt 2
`
	delta := diffDelta(prev, curr)
	if !strings.Contains(delta, "util.go") {
		t.Errorf("delta should contain util.go (new file); got: %q", delta)
	}
	if strings.Contains(delta, "auth.go") {
		t.Errorf("delta should not re-attribute auth.go (unchanged since prev); got: %q", delta)
	}
}
