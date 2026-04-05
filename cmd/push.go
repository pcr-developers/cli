package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"sort"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/config"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/store"
	"github.com/pcr-developers/cli/internal/supabase"
)

var pushCmd = &cobra.Command{
	Use:   "push",
	Short: "Push sealed prompt bundles to PCR.dev for review",
	RunE: func(cmd *cobra.Command, args []string) error {
		return runManualPush()
	},
}

// runManualPush is the default: push all unpushed bundles, sealing any open ones first.
func runManualPush() error {
	a := auth.Load()
	if a == nil {
		fmt.Fprintln(os.Stderr, "PCR: Not logged in. Run `pcr login`.")
		return nil
	}

	allUnpushed, err := store.GetUnpushedCommits()
	if err != nil {
		return err
	}
	if len(allUnpushed) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No prompt bundles to push. Run `pcr bundle \"name\" --select all` first.")
		return nil
	}

	// Auto-seal open bundles — running pcr push means the session is done.
	var commits []store.PromptCommit
	for _, c := range allUnpushed {
		if c.BundleStatus == "open" {
			if err := store.CloseBundle(c.ID); err != nil {
				return err
			}
			c.BundleStatus = "closed"
			fmt.Fprintf(os.Stderr, "PCR: Sealed %q\n", c.Message)
		}
		commits = append(commits, c)
	}

	pushed := 0
	currentBranch := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
	for _, commit := range commits {
		pushed += pushBundle(commit.ID, currentBranch, a.UserID)
	}
	if pushed == 0 {
		fmt.Fprintln(os.Stderr, "PCR: Nothing new pushed.")
	}
	return nil
}

// pushBundle pushes a single sealed bundle by local ID and returns 1 on success.
func pushBundle(localID, currentBranch, userID string) int {
	c, err := store.GetCommitWithItems(localID)
	if err != nil || c == nil {
		return 0
	}

	source := dominantSource(c.Items)
	touchedProjects := collectTouchedProjects(c.Items)

	remoteID, err := supabase.UpsertBundle("", supabase.BundleData{
		BundleID:        c.ID,
		Message:         c.Message,
		Source:          source,
		ProjectName:     c.ProjectName,
		SessionShas:     c.SessionShas,
		HeadSha:         c.HeadSha,
		ExchangeCount:   len(c.Items),
		CommittedAt:     c.CommittedAt,
		TouchedProjects: touchedProjects,
	}, userID)
	if err != nil {
		fmt.Fprintf(os.Stderr, "PCR: Failed to push prompt bundle %q: %v\n", c.Message, err)
		return 0
	}

	// Compute incremental diffs: for each session group sorted by captured_at,
	// show only what changed since the previous prompt (commit diff or working-tree delta).
	incrementalDiffs := computeIncrementalDiffs(c.Items)

	var promptRecords []map[string]any
	var diffRecords []map[string]any
	for _, item := range c.Items {
		// project_ids: all repos this specific prompt touched (from DiffTracker).
		// This is the per-prompt attribution — independent of the bundle's bundle_projects.
		promptProjectIDs := item.TouchedProjectIDs()
		if len(promptProjectIDs) == 0 && item.ProjectID != "" {
			promptProjectIDs = []string{item.ProjectID}
		}

		rec := map[string]any{
			"id":             item.ID,
			"content_hash":   item.ContentHash,
			"bundle_id":      c.ID,
			"session_id":     item.SessionID,
			"prompt_text":    item.PromptText,
			"tool_calls":     item.ToolCalls,
			"model":          item.Model,
			"source":         item.Source,
			"branch_name":    item.BranchName,
			"captured_at":    item.CapturedAt,
			"capture_method": item.CaptureMethod,
			"project_ids":    promptProjectIDs,
		}
		if item.ProjectID != "" {
			rec["project_id"] = item.ProjectID
		}
		if item.ResponseText != "" {
			rec["response_text"] = item.ResponseText
		}
		if len(item.FileContext) > 0 {
			rec["file_context"] = item.FileContext
		}
		promptRecords = append(promptRecords, rec)
		if diff := incrementalDiffs[item.ID]; diff != "" {
			diffRecords = append(diffRecords, map[string]any{
				"prompt_id": item.ID,
				"diff":      diff,
			})
		}
	}
	if err := supabase.UpsertBundlePrompts("", promptRecords, diffRecords, userID); err != nil {
		fmt.Fprintf(os.Stderr, "PCR: Warning — prompt bundle pushed but prompts failed: %v\n", err)
	}

	if remoteID == "" {
		remoteID = c.ID
	}
	if err := store.MarkPushed(c.ID, remoteID); err != nil {
		fmt.Fprintf(os.Stderr, "PCR: Warning — pushed but failed to mark locally: %v\n", err)
	}

	reviewURL := config.AppURL + "/review/" + remoteID
	branch := c.BranchName
	if branch == "" {
		branch = currentBranch
	}
	fmt.Fprintf(os.Stderr, "PCR: Pushed %q (%d prompt%s)\n", c.Message, len(c.Items), plural(len(c.Items)))
	if branch != "" {
		fmt.Fprintf(os.Stderr, "    Branch:  %s\n", branch)
	}
	fmt.Fprintf(os.Stderr, "    Review:  %s\n", reviewURL)
	if prURL := detectGitHubPR(); prURL != "" {
		fmt.Fprintf(os.Stderr, "    PR:      %s\n", prURL)
	}
	return 1
}


// collectTouchedProjects gathers every project a bundle touched, with the
// current branch for each repo looked up from the local git working tree.
// The first project is marked is_primary=true.
func collectTouchedProjects(items []store.DraftRecord) []supabase.TouchedProject {
	// Count hits per project to determine primary
	hits := map[string]int{}
	for _, item := range items {
		if item.ProjectID != "" {
			hits[item.ProjectID]++
		}
		for _, id := range item.TouchedProjectIDs() {
			hits[id]++
		}
	}
	if len(hits) == 0 {
		return nil
	}

	// Build project registry for branch lookup
	projByID := map[string]string{} // id → path
	for _, p := range projects.Load() {
		if p.ProjectID != "" {
			projByID[p.ProjectID] = p.Path
		}
	}

	// Sort by hit count desc to identify primary
	type entry struct{ id string; count int }
	var sorted []entry
	for id, count := range hits {
		sorted = append(sorted, entry{id, count})
	}
	for i := 0; i < len(sorted)-1; i++ {
		for j := i + 1; j < len(sorted); j++ {
			if sorted[j].count > sorted[i].count {
				sorted[i], sorted[j] = sorted[j], sorted[i]
			}
		}
	}

	var result []supabase.TouchedProject
	for i, e := range sorted {
		branch := ""
		if path, ok := projByID[e.id]; ok && path != "" {
			branch = gitOutputIn(path, "git", "rev-parse", "--abbrev-ref", "HEAD")
		}
		result = append(result, supabase.TouchedProject{
			ProjectID: e.id,
			Branch:    branch,
			IsPrimary: i == 0,
		})
	}
	return result
}

// computeIncrementalDiffs returns a map of prompt ID → incremental diff.
// For each (session, repo) timeline sorted by captured_at:
//   - First prompt: raw gitDiff filtered to tool-call files
//   - HEAD changed: git diff <prev>..<curr> -- <tool-call files>
//   - Same HEAD: working-tree delta filtered to tool-call files
// Secondary repos (from file_context.repo_snapshots) are appended after
// the primary repo diff, giving a complete multi-repo picture per prompt.
func computeIncrementalDiffs(items []store.DraftRecord) map[string]string {
	// Build project path lookup
	projByID := map[string]string{} // id → path
	for _, p := range projects.Load() {
		if p.ProjectID != "" {
			projByID[p.ProjectID] = p.Path
		}
	}

	type repoKey struct{ sessionID, projectID string }
	type repoPrompt struct {
		itemID     string
		capturedAt string
		headSha    string
		gitDiff    string
		toolFiles  []string // relative paths in this repo the AI touched
	}

	timelines := map[repoKey][]repoPrompt{}
	// Track which projectID is primary for each session (first item wins).
	primaryProjBySession := map[string]string{}

	for _, item := range items {
		if _, ok := primaryProjBySession[item.SessionID]; !ok {
			primaryProjBySession[item.SessionID] = item.ProjectID
		}

		// Primary repo
		pk := repoKey{item.SessionID, item.ProjectID}
		timelines[pk] = append(timelines[pk], repoPrompt{
			itemID:     item.ID,
			capturedAt: item.CapturedAt,
			headSha:    item.HeadSha,
			gitDiff:    item.GitDiff,
			toolFiles:  tcFilesForProject(item.ToolCalls, projByID[item.ProjectID]),
		})

		// Secondary repos stored at capture time in file_context.repo_snapshots
		if snapsRaw, ok := item.FileContext["repo_snapshots"]; ok {
			if snapsMap, ok := snapsRaw.(map[string]any); ok {
				for repoID, snapRaw := range snapsMap {
					snap, ok := snapRaw.(map[string]any)
					if !ok {
						continue
					}
					headSha, _ := snap["head_sha"].(string)
					gitDiff, _ := snap["git_diff"].(string)
					sk := repoKey{item.SessionID, repoID}
					timelines[sk] = append(timelines[sk], repoPrompt{
						itemID:     item.ID,
						capturedAt: item.CapturedAt,
						headSha:    headSha,
						gitDiff:    gitDiff,
						toolFiles:  tcFilesForProject(item.ToolCalls, projByID[repoID]),
					})
				}
			}
		}
	}

	primaryDiffs := map[string]string{}
	secondaryDiffs := map[string][]string{}

	for key, timeline := range timelines {
		sort.Slice(timeline, func(i, j int) bool {
			return timeline[i].capturedAt < timeline[j].capturedAt
		})

		projectPath := projByID[key.projectID]
		isPrimary := primaryProjBySession[key.sessionID] == key.projectID

		for i, data := range timeline {
			var diff string
			if i == 0 {
				// First prompt: filter raw snapshot to tool-call files if available.
				if len(data.toolFiles) > 0 {
					diff = filterDiffToFiles(data.gitDiff, data.toolFiles)
				} else {
					diff = data.gitDiff
				}
			} else {
				prev := timeline[i-1]
				if data.headSha != "" && prev.headSha != "" && data.headSha != prev.headSha {
					// HEAD advanced — show only committed changes for tool-call files.
					args := []string{"-C", projectPath, "diff", prev.headSha + ".." + data.headSha}
					if len(data.toolFiles) > 0 {
						args = append(args, "--")
						args = append(args, data.toolFiles...)
					}
					if out, err := exec.Command("git", args...).Output(); err == nil && len(out) > 0 {
						diff = truncateDiff(string(out))
					}
				} else {
					// Same HEAD — working-tree delta, scoped to tool-call files.
					rawDelta := diffDelta(prev.gitDiff, data.gitDiff)
					if len(data.toolFiles) > 0 {
						diff = filterDiffToFiles(rawDelta, data.toolFiles)
					} else {
						diff = rawDelta
					}
				}
			}

			if diff == "" {
				continue
			}
			if isPrimary {
				primaryDiffs[data.itemID] = diff
			} else {
				secondaryDiffs[data.itemID] = append(secondaryDiffs[data.itemID], diff)
			}
		}
	}

	// Combine primary + secondary diffs per prompt.
	result := map[string]string{}
	seen := map[string]bool{}
	for id, d := range primaryDiffs {
		seen[id] = true
		parts := []string{d}
		parts = append(parts, secondaryDiffs[id]...)
		result[id] = strings.Join(parts, "")
	}
	for id, secs := range secondaryDiffs {
		if seen[id] {
			continue
		}
		result[id] = strings.Join(secs, "")
	}
	return result
}

// tcFilesForProject returns relative file paths from tool calls that fall under
// projectPath, deduped and in first-seen order.
func tcFilesForProject(toolCalls []map[string]any, projectPath string) []string {
	if projectPath == "" || len(toolCalls) == 0 {
		return nil
	}
	seen := map[string]bool{}
	var files []string
	for _, tc := range toolCalls {
		abs := tcPath(tc)
		if abs == "" || !strings.HasPrefix(abs, projectPath+"/") {
			continue
		}
		rel := strings.TrimPrefix(abs, projectPath+"/")
		if !seen[rel] {
			seen[rel] = true
			files = append(files, rel)
		}
	}
	return files
}

// tcPath extracts the absolute file path from a tool call map.
func tcPath(tc map[string]any) string {
	if input, ok := tc["input"].(map[string]any); ok {
		if p, ok := input["path"].(string); ok && p != "" {
			return p
		}
		if p, ok := input["file_path"].(string); ok && p != "" {
			return p
		}
	}
	if p, ok := tc["path"].(string); ok {
		return p
	}
	return ""
}

// filterDiffToFiles returns only diff sections for files in relFiles (relative to project root).
func filterDiffToFiles(diff string, relFiles []string) string {
	if diff == "" || len(relFiles) == 0 {
		return diff
	}
	fileSet := map[string]bool{}
	for _, f := range relFiles {
		fileSet[f] = true
	}
	var result []string
	for _, section := range splitDiffSections(diff) {
		header := diffFileHeader(section)
		for _, field := range strings.Fields(header) {
			if strings.HasPrefix(field, "b/") && fileSet[strings.TrimPrefix(field, "b/")] {
				result = append(result, section)
				break
			}
		}
	}
	return strings.Join(result, "")
}

// splitDiffSections splits a unified diff into per-file sections in order.
func splitDiffSections(diff string) []string {
	if diff == "" {
		return nil
	}
	var starts []int
	if strings.HasPrefix(diff, "diff --git ") {
		starts = append(starts, 0)
	}
	idx := 0
	for {
		pos := strings.Index(diff[idx:], "\ndiff --git ")
		if pos < 0 {
			break
		}
		starts = append(starts, idx+pos+1)
		idx += pos + 1
	}
	sections := make([]string, len(starts))
	for i, start := range starts {
		end := len(diff)
		if i+1 < len(starts) {
			end = starts[i+1]
		}
		sections[i] = diff[start:end]
	}
	return sections
}

// diffDelta returns the portions of currDiff that are new or changed vs prevDiff,
// compared on a per-file basis using unified diff section headers.
func diffDelta(prevDiff, currDiff string) string {
	if currDiff == "" {
		return ""
	}
	if prevDiff == "" {
		return currDiff
	}
	prevSections := splitDiffByFile(prevDiff)
	var result []string
	for _, section := range splitDiffSections(currDiff) {
		header := diffFileHeader(section)
		if prev, ok := prevSections[header]; !ok || prev != section {
			result = append(result, section)
		}
	}
	return strings.Join(result, "")
}

// splitDiffByFile splits a unified diff into per-file sections keyed by header.
func splitDiffByFile(diff string) map[string]string {
	sections := map[string]string{}
	for _, section := range splitDiffSections(diff) {
		sections[diffFileHeader(section)] = section
	}
	return sections
}

// diffFileHeader extracts the first line of a diff section (the "diff --git ..." line).
func diffFileHeader(section string) string {
	if nl := strings.Index(section, "\n"); nl >= 0 {
		return section[:nl]
	}
	return section
}

func truncateDiff(diff string) string {
	const maxBytes = 50_000
	if len(diff) > maxBytes {
		return diff[:maxBytes] + "\n[truncated]"
	}
	return diff
}

// dominantSource returns the most common source string among bundle items.
// Falls back to "unknown" if items is empty.
func dominantSource(items []store.DraftRecord) string {
	counts := map[string]int{}
	for _, item := range items {
		if item.Source != "" {
			counts[item.Source]++
		}
	}
	best, bestN := "unknown", 0
	for src, n := range counts {
		if n > bestN {
			best, bestN = src, n
		}
	}
	return best
}

// detectGitHubPR tries to find the GitHub PR URL for the current branch using `gh`.
func detectGitHubPR() string {
	out, err := exec.Command("gh", "pr", "view", "--json", "url", "-q", ".url").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}
