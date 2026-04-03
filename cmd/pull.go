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

var pullCmd = &cobra.Command{
	Use:   "pull [remote-id]",
	Short: "Restore a pushed bundle to local drafts",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		remoteID := args[0]

		a := auth.Load()
		if a == nil {
			return fmt.Errorf("not logged in — run `pcr login`")
		}

		bundle, err := supabase.PullBundle(a.Token, remoteID)
		if err != nil {
			return fmt.Errorf("failed to fetch bundle: %w", err)
		}

		// The bundle contains an items array of PromptRecords
		itemsRaw, _ := bundle["items"]
		itemsJSON, _ := json.Marshal(itemsRaw)
		var items []supabase.PromptRecord
		if err := json.Unmarshal(itemsJSON, &items); err != nil {
			return fmt.Errorf("failed to parse bundle items: %w", err)
		}

		restored := 0
		for _, item := range items {
			if err := store.SaveDraft(item, nil); err == nil {
				restored++
			}
		}

		fmt.Fprintf(os.Stderr, "PCR: Restored %d prompt%s from bundle %s\n",
			restored, plural(restored), remoteID)
		return nil
	},
}
