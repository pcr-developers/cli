package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/store"
	"github.com/pcr-developers/cli/internal/supabase"
)

var pullCmd = &cobra.Command{
	Use:   "pull [remote-id]",
	Short: "Restore a pushed bundle to local drafts",
	Args:  cobra.MaximumNArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		a := auth.Load()
		if a == nil {
			return fmt.Errorf("not logged in — run `pcr login`")
		}

		remoteID := ""
		if len(args) == 1 {
			remoteID = strings.TrimSpace(args[0])
		}

		// If no remote ID, list pushed bundles and let user pick
		if remoteID == "" {
			pushed, err := store.ListPushedCommits()
			if err != nil {
				return err
			}
			if len(pushed) == 0 {
				fmt.Fprintln(os.Stderr, "PCR: No pushed bundles found.")
				return nil
			}

			fmt.Fprintf(os.Stderr, "Pushed bundles:\n\n")
			for i, b := range pushed {
				fmt.Fprintf(os.Stderr, "  [%d] %q  remote: %s\n", i+1, b.Message, b.RemoteID)
			}
			fmt.Fprintln(os.Stderr)

			tty := openTTY()
			defer tty.Close()

			resp := strings.TrimSpace(ttyPrompt(tty, "Select bundle to pull [number]: "))
			idx := parseFirstIndex(resp, len(pushed))
			if idx < 0 {
				fmt.Fprintln(os.Stderr, "PCR: Nothing pulled.")
				return nil
			}
			remoteID = pushed[idx].RemoteID
		}

		if remoteID == "" {
			return fmt.Errorf("no remote ID")
		}

		bundle, err := supabase.PullBundle(a.Token, remoteID)
		if err != nil {
			return fmt.Errorf("failed to fetch bundle: %w", err)
		}

		itemsRaw, _ := bundle["items"]
		itemsJSON, _ := json.Marshal(itemsRaw)
		var items []supabase.PromptRecord
		if err := json.Unmarshal(itemsJSON, &items); err != nil {
			return fmt.Errorf("failed to parse bundle items: %w", err)
		}

		restored := 0
		for _, item := range items {
			if err := store.SaveDraft(item, nil, ""); err == nil {
				restored++
			}
		}

		fmt.Fprintf(os.Stderr, "PCR: Restored %d prompt%s from bundle %s\n",
			restored, plural(restored), remoteID)
		return nil
	},
}
