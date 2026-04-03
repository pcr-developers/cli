package cmd

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/store"
	"github.com/pcr-developers/cli/internal/supabase"
)

func nullableStr(s string) any {
	if s == "" {
		return nil
	}
	return s
}

var pushCmd = &cobra.Command{
	Use:   "push",
	Short: "Upload committed bundles to PCR.dev",
	RunE: func(cmd *cobra.Command, args []string) error {
		a := auth.Load()
		if a == nil {
			fmt.Fprintln(os.Stderr, "PCR: Not logged in. Run `pcr login`.")
			return nil
		}

		commits, err := store.GetUnpushedCommits()
		if err != nil {
			return err
		}
		if len(commits) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No committed bundles to push. Run `pcr commit` first.")
			return nil
		}

		pushed := 0
		for _, commit := range commits {
			c, err := store.GetCommitWithItems(commit.ID)
			if err != nil || c == nil {
				continue
			}

			// Serialize items for the RPC
			itemsJSON, _ := json.Marshal(c.Items)
			var items []map[string]any
			_ = json.Unmarshal(itemsJSON, &items)

			remoteID, err := supabase.UpsertPromptBundle(a.Token, map[string]any{
				"p_bundle": map[string]any{
					"id":           c.ID,
					"message":      c.Message,
					"project_id":   nullableStr(c.ProjectID),
					"project_name": nullableStr(c.ProjectName),
					"branch_name":  nullableStr(c.BranchName),
					"session_shas": c.SessionShas,
					"head_sha":     c.HeadSha,
					"committed_at": c.CommittedAt,
					"items":        items,
				},
				"p_user_id": a.UserID,
			})
			if err != nil {
				fmt.Fprintf(os.Stderr, "PCR: Failed to push bundle %q: %v\n", c.Message, err)
				continue
			}

			if remoteID == "" {
				remoteID = c.ID
			}
			if err := store.MarkPushed(c.ID, remoteID); err != nil {
				fmt.Fprintf(os.Stderr, "PCR: Warning — pushed but failed to mark locally: %v\n", err)
			}

			fmt.Fprintf(os.Stderr, "PCR: Pushed bundle %q (%d prompt%s)\n",
				c.Message, len(c.Items), plural(len(c.Items)))
			pushed++
		}

		if pushed == 0 {
			fmt.Fprintln(os.Stderr, "PCR: Nothing new pushed.")
		}
		return nil
	},
}
