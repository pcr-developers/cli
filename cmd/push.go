package cmd

import (
	"fmt"
	"os"
	"os/exec"
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

// runManualPush is the default: push already-sealed bundles.
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
		fmt.Fprintln(os.Stderr, "PCR: No committed prompt bundles to push. Run `pcr commit` first.")
		fmt.Fprintln(os.Stderr, "     Or use `pcr push --auto` to auto-bundle drafts by git commit.")
		return nil
	}

	var commits []store.PromptCommit
	var openCount int
	for _, c := range allUnpushed {
		if c.BundleStatus == "open" {
			openCount++
		} else {
			commits = append(commits, c)
		}
	}
	if openCount > 0 {
		fmt.Fprintf(os.Stderr, "PCR: Skipping %d open prompt bundle%s — seal with `pcr commit \"name\"` first.\n",
			openCount, plural(openCount))
	}
	if len(commits) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No sealed prompt bundles to push.")
		return nil
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
	}, c.ProjectID, userID)
	if err != nil {
		fmt.Fprintf(os.Stderr, "PCR: Failed to push prompt bundle %q: %v\n", c.Message, err)
		return 0
	}

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
		if item.GitDiff != "" {
			diffRecords = append(diffRecords, map[string]any{
				"prompt_id": item.ID,
				"diff":      item.GitDiff,
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
