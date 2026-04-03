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
	Short: "Push sealed bundles to PCR.dev for review",
	RunE: func(cmd *cobra.Command, args []string) error {
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
			fmt.Fprintln(os.Stderr, "PCR: No committed bundles to push. Run `pcr commit` first.")
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
			fmt.Fprintf(os.Stderr, "PCR: Skipping %d open bundle%s — seal with `pcr commit \"name\"` first.\n",
				openCount, plural(openCount))
		}
		if len(commits) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No sealed bundles to push.")
			return nil
		}

		currentBranch := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
		pushed := 0
		for _, commit := range commits {
			c, err := store.GetCommitWithItems(commit.ID)
			if err != nil || c == nil {
				continue
			}

			remoteID, err := supabase.UpsertClaudeBundle("", supabase.ClaudeBundleData{
				BundleID:      c.ID,
				Message:       c.Message,
				ProjectName:   c.ProjectName,
				BranchName:    c.BranchName,
				SessionShas:   c.SessionShas,
				HeadSha:       c.HeadSha,
				ExchangeCount: len(c.Items),
				CommittedAt:   c.CommittedAt,
			}, c.ProjectID, a.UserID)
			if err != nil {
				fmt.Fprintf(os.Stderr, "PCR: Failed to push bundle %q: %v\n", c.Message, err)
				continue
			}

			// Push individual prompts and git diffs
			var promptRecords []map[string]any
			var diffRecords []map[string]any
			for _, item := range c.Items {
				rec := map[string]any{
					"id":           item.ID,
					"content_hash": item.ContentHash,
					"bundle_id":    c.ID,
					"session_id":   item.SessionID,
					"prompt_text":  item.PromptText,
					"tool_calls":   item.ToolCalls,
					"model":        item.Model,
					"source":       item.Source,
					"branch_name":  item.BranchName,
					"captured_at":  item.CapturedAt,
				}
				if item.ProjectID != "" {
					rec["project_id"] = item.ProjectID
				}
				if item.ResponseText != "" {
					rec["response_text"] = item.ResponseText
				}
				promptRecords = append(promptRecords, rec)
				if item.GitDiff != "" {
					diffRecords = append(diffRecords, map[string]any{
						"prompt_id": item.ID,
						"diff":      item.GitDiff,
					})
				}
			}
			if err := supabase.UpsertBundlePrompts("", promptRecords, diffRecords, a.UserID); err != nil {
				fmt.Fprintf(os.Stderr, "PCR: Warning — bundle metadata pushed but prompts failed: %v\n", err)
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
			pushed++
		}

		if pushed == 0 {
			fmt.Fprintln(os.Stderr, "PCR: Nothing new pushed.")
		}
		return nil
	},
}

// detectGitHubPR tries to find the GitHub PR URL for the current branch using `gh`.
func detectGitHubPR() string {
	out, err := exec.Command("gh", "pr", "view", "--json", "url", "-q", ".url").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}
