package cmd

import (
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/store"
)

var addCmd = &cobra.Command{
	Use:   "add",
	Short: "Stage draft prompts for bundling",
	Long: `Interactively select draft prompts to stage for a manual bundle.
After staging, run 'pcr commit -m "message"' to create the bundle.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		clearFlag, _ := cmd.Flags().GetBool("clear")

		if clearFlag {
			if err := store.ClearStaged(); err != nil {
				return err
			}
			fmt.Fprintln(os.Stderr, "PCR: Cleared all staged prompts.")
			return nil
		}

		// Show what's already staged
		staged, err := store.GetStagedDrafts()
		if err != nil {
			return err
		}
		if len(staged) > 0 {
			fmt.Fprintf(os.Stderr, "PCR: %d prompt%s already staged:\n", len(staged), plural(len(staged)))
			for i, d := range staged {
				fmt.Fprintf(os.Stderr, "  [%d] %q\n", i+1, truncate(d.PromptText, 72))
			}
			fmt.Fprintln(os.Stderr)
		}

		// Show available drafts
		drafts, err := store.GetDraftsByStatus(store.StatusDraft, nil, nil)
		if err != nil {
			return err
		}
		if len(drafts) == 0 {
			if len(staged) > 0 {
				fmt.Fprintln(os.Stderr, "PCR: No more draft prompts available. Run `pcr commit -m \"message\"` to bundle.")
			} else {
				fmt.Fprintln(os.Stderr, "PCR: No draft prompts available. Start `pcr start` to capture prompts.")
			}
			return nil
		}

		const (
			grn  = "\x1b[32m"
			bold = "\x1b[1m"
			rst  = "\x1b[0m"
		)

		fmt.Fprintf(os.Stderr, "%s%sDRAFT PROMPTS%s\n\n", grn, bold, rst)
		for i, d := range drafts {
			fmt.Fprintf(os.Stderr, "  [%d] %q\n", i+1, truncate(d.PromptText, 72))
		}
		fmt.Fprintln(os.Stderr)

		tty := openTTY()
		defer tty.Close()

		resp := ttyPrompt(tty, "Select prompts to stage [e.g. 1,2 or all — enter to skip]: ")
		resp = strings.TrimSpace(resp)
		if resp == "" {
			fmt.Fprintln(os.Stderr, "PCR: Nothing staged.")
			return nil
		}

		var selected []store.DraftRecord
		if strings.ToLower(resp) == "all" {
			selected = drafts
		} else {
			selected = parseSelection(resp, drafts)
		}

		if len(selected) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing staged.")
			return nil
		}

		if err := store.StageDrafts(draftIDs(selected)); err != nil {
			return err
		}

		fmt.Fprintf(os.Stderr, "PCR: Staged %d prompt%s. Run `pcr commit -m \"message\"` to bundle.\n",
			len(selected), plural(len(selected)))
		return nil
	},
}

func init() {
	addCmd.Flags().Bool("clear", false, "Unstage all staged prompts")
}
