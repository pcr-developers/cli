package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/config"
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
	remoteID, err := supabase.UpsertBundle("", supabase.BundleData{
		BundleID:          c.ID,
		Message:           c.Message,
		Source:            source,
		ProjectName:       c.ProjectName,
		BranchName:        c.BranchName,
		SessionShas:       c.SessionShas,
		HeadSha:           c.HeadSha,
		ExchangeCount:     len(c.Items),
		CommittedAt:       c.CommittedAt,
		TouchedProjectIDs: collectTouchedProjectIDs(c.Items),
	}, c.ProjectID, userID)
	if err != nil {
		fmt.Fprintf(os.Stderr, "PCR: Failed to push prompt bundle %q: %v\n", c.Message, err)
		return 0
	}

	var promptRecords []map[string]any
	var diffRecords []map[string]any
	for _, item := range c.Items {
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
		}
		if item.ProjectID != "" {
			rec["project_id"] = item.ProjectID
		}
		if item.ResponseText != "" {
			rec["response_text"] = item.ResponseText
		}
		// file_context carries touched_project_ids, relevant_files, cursor_mode,
		// is_agentic, capture_schema — all needed for per-prompt repo display in the UI.
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


// collectTouchedProjectIDs gathers every project ID mentioned across a bundle's
// prompts — the primary project_id of each prompt plus any additional IDs
// stored in file_context.touched_project_ids for cross-repo prompts.
// The result is deduplicated and safe to send as p_touched_project_ids.
func collectTouchedProjectIDs(items []store.DraftRecord) []string {
	seen := map[string]bool{}
	var result []string
	add := func(id string) {
		if id != "" && !seen[id] {
			seen[id] = true
			result = append(result, id)
		}
	}
	for _, item := range items {
		add(item.ProjectID)
		for _, id := range item.TouchedProjectIDs() {
			add(id)
		}
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
