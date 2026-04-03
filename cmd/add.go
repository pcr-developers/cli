package cmd

import (
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/store"
)

var addCmd = &cobra.Command{
	Use:   "add [bundle-name]",
	Short: "Browse bundles and drafts, or add drafts to a bundle",
	Long: `With no arguments: shows all bundles with a numbered list — pick one to
add prompts, remove prompts, rename, or seal it. Falls through to staging
new drafts if you skip the bundle selection.

With a bundle name: adds captured drafts directly to that bundle (creates it if new).

Flags:
  --remove "bundle"  Remove prompts from a bundle
  --clear            Unstage all staged prompts

Examples:
  pcr add                   # browse bundles, pick one to edit
  pcr add "auth refactor"   # add drafts to "auth refactor"
  pcr add --remove "auth refactor"  # remove prompts from a bundle`,
	RunE: func(cmd *cobra.Command, args []string) error {
		clearFlag, _ := cmd.Flags().GetBool("clear")

		if clearFlag {
			if err := store.ClearStaged(); err != nil {
				return err
			}
			fmt.Fprintln(os.Stderr, "PCR: Cleared all staged prompts.")
			return nil
		}

		bundleName := ""
		if len(args) > 0 {
			bundleName = strings.TrimSpace(strings.Join(args, " "))
		}

		deleteFlag, _ := cmd.Flags().GetBool("delete")
		if deleteFlag {
			return runDeleteDrafts()
		}

		removeFlag, _ := cmd.Flags().GetBool("remove")
		if removeFlag {
			if bundleName == "" {
				return fmt.Errorf("--remove requires a bundle name: pcr add --remove \"bundle-name\"")
			}
			return runRemoveFromBundle(bundleName)
		}

		if bundleName != "" {
			return runAddToBundle(bundleName)
		}
		return runStage()
	},
}

// runAddToBundle adds selected drafts to a named open bundle (creating it if needed).
// If the bundle already exists, it shows what's already in it first.
func runAddToBundle(bundleName string) error {
	ctx := resolveProjectContext()

	bundle, err := store.GetBundleByName(bundleName)
	if err != nil {
		return err
	}

	// Show existing bundle contents if it already exists
	if bundle != nil {
		full, err := store.GetCommitWithItems(bundle.ID)
		if err != nil {
			return err
		}
		if len(full.Items) > 0 {
			const dim = "\x1b[2m"
			const rst = "\x1b[0m"
			fmt.Fprintf(os.Stderr, "PCR: Bundle %q already has %d prompt%s:\n\n", bundleName, len(full.Items), plural(len(full.Items)))
			for _, d := range full.Items {
				date := formatCapturedAt(d.CapturedAt)
				fmt.Fprintf(os.Stderr, "  [already in bundle] %s%s%s %q\n", dim, date, rst, truncate(d.PromptText, 60))
			}
			fmt.Fprintln(os.Stderr)
		}
	}

	drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
	if err != nil {
		return err
	}
	staged, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	all := append(drafts, staged...)

	if len(all) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No draft prompts available to add.")
		return nil
	}

	const dim = "\x1b[2m"
	const rst = "\x1b[0m"
	fmt.Fprintf(os.Stderr, "PCR: %d draft prompt%s available to add:\n\n", len(all), plural(len(all)))
	for idx, d := range all {
		date := formatCapturedAt(d.CapturedAt)
		fmt.Fprintf(os.Stderr, "  [%d] %s%s%s %q\n", idx+1, dim, date, rst, truncate(d.PromptText, 60))
	}
	fmt.Fprintln(os.Stderr)

	tty := openTTY()
	defer tty.Close()

	resp := ttyPrompt(tty, "Select prompts to add [e.g. 1,2 or all — enter to skip]: ")
	resp = strings.TrimSpace(resp)
	if resp == "" {
		fmt.Fprintln(os.Stderr, "PCR: Nothing added.")
		return nil
	}

	var selected []store.DraftRecord
	if strings.ToLower(resp) == "all" {
		selected = all
	} else {
		selected = parseSelection(resp, all)
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing added.")
		return nil
	}

	ids := draftIDs(selected)

	if bundle != nil {
		if err := store.AddDraftsToBundle(bundle.ID, ids); err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "PCR: Added %d prompt%s to %q — seal with `pcr commit %q`\n",
			len(selected), plural(len(selected)), bundleName, bundleName)
	} else {
		projectID := ""
		projectName := ctx.name
		if len(ctx.ids) > 0 {
			projectID = ctx.ids[0]
		}
		branch := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
		syntheticSha := "manual-" + generateID()
		_, err := store.CreateCommit(bundleName, syntheticSha, ids, projectID, projectName, branch, "open")
		if err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "PCR: Created bundle %q with %d prompt%s — seal with `pcr commit %q`\n",
			bundleName, len(selected), plural(len(selected)), bundleName)
	}
	return nil
}

// runStage shows all unpushed bundles, lets the user pick one to edit,
// or falls through to staging new drafts.
func runStage() error {
	ctx := resolveProjectContext()

	unpushed, err := store.GetUnpushedCommits()
	if err != nil {
		return err
	}

	if len(unpushed) > 0 {
		const bold = "\x1b[1m"
		const dim = "\x1b[2m"
		const rst = "\x1b[0m"
		fmt.Fprintf(os.Stderr, "%sBUNDLES%s\n\n", bold, rst)
		for i, b := range unpushed {
			full, _ := store.GetCommitWithItems(b.ID)
			count := 0
			if full != nil {
				count = len(full.Items)
			}
			status := "open  "
			style := ""
			styleEnd := ""
			if b.BundleStatus != "open" {
				status = "sealed"
				style = dim
				styleEnd = rst
			}
			fmt.Fprintf(os.Stderr, "  %s[%d] [%s] %q (%d prompt%s)%s\n", style, i+1, status, b.Message, count, plural(count), styleEnd)
		}
		fmt.Fprintln(os.Stderr)

		tty := openTTY()
		defer tty.Close()

		resp := strings.TrimSpace(ttyPrompt(tty, "Select a bundle to edit [number], or press enter to stage new drafts: "))
		if resp != "" {
			idx := parseFirstIndex(resp, len(unpushed))
			if idx >= 0 {
				return runEditBundle(unpushed[idx], tty)
			}
		}
	}

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

	drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
	if err != nil {
		return err
	}
	if len(drafts) == 0 {
		if len(staged) > 0 {
			fmt.Fprintln(os.Stderr, "PCR: No more drafts available. Run `pcr commit -m \"message\"` to bundle staged prompts.")
		} else {
			fmt.Fprintln(os.Stderr, "PCR: No draft prompts available. Run `pcr start` to capture prompts.")
		}
		return nil
	}

	const (
		grn  = "\x1b[32m"
		bold = "\x1b[1m"
		dim  = "\x1b[2m"
		rst  = "\x1b[0m"
	)

	fmt.Fprintf(os.Stderr, "%s%sDRAFT PROMPTS%s\n\n", grn, bold, rst)
	for i, d := range drafts {
		date := formatCapturedAt(d.CapturedAt)
		fmt.Fprintf(os.Stderr, "  [%d] %s%s%s %q\n", i+1, dim, date, rst, truncate(d.PromptText, 60))
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
}

// openBundlesAsDrafts is a shim so we can reuse parseSelection with bundles.
func openBundlesAsDrafts(bundles []store.PromptCommit) []store.DraftRecord {
	out := make([]store.DraftRecord, len(bundles))
	for i, b := range bundles {
		out[i] = store.DraftRecord{ID: b.ID, PromptText: b.Message}
	}
	return out
}

// parseFirstIndex parses a single 1-based number from the response string.
func parseFirstIndex(resp string, max int) int {
	fields := strings.FieldsFunc(resp, func(r rune) bool {
		return r == ',' || r == ' '
	})
	if len(fields) == 0 {
		return -1
	}
	var n int
	if _, err := fmt.Sscanf(fields[0], "%d", &n); err != nil || n < 1 || n > max {
		return -1
	}
	return n - 1
}

// runEditBundle shows a selected bundle's contents and offers add/remove/rename/seal actions.
func runEditBundle(b store.PromptCommit, tty *ttyHandle) error {
	const bold = "\x1b[1m"
	const dim = "\x1b[2m"
	const rst = "\x1b[0m"

	full, err := store.GetCommitWithItems(b.ID)
	if err != nil {
		return err
	}

	fmt.Fprintf(os.Stderr, "\n%s%q%s\n\n", bold, b.Message, rst)
	if len(full.Items) == 0 {
		fmt.Fprintf(os.Stderr, "  (no prompts yet)\n\n")
	} else {
		for i, d := range full.Items {
			date := formatCapturedAt(d.CapturedAt)
			fmt.Fprintf(os.Stderr, "  [%d] %s%s%s %q\n", i+1, dim, date, rst, truncate(d.PromptText, 60))
		}
		fmt.Fprintln(os.Stderr)
	}

	actions := "  [a] add prompts   [r] remove prompts   [n] rename   [d] delete bundle"
	if b.BundleStatus == "open" {
		actions += "   [s] seal"
	}
	fmt.Fprintln(os.Stderr, actions)
	fmt.Fprintln(os.Stderr)

	action := strings.ToLower(strings.TrimSpace(ttyPrompt(tty, "Action [a/r/n/d/s — enter to cancel]: ")))
	switch action {
	case "a":
		return runAddToBundle(b.Message)
	case "r":
		return runRemoveFromBundle(b.Message)
	case "n":
		newName := strings.TrimSpace(ttyPrompt(tty, "New name: "))
		if newName == "" {
			fmt.Fprintln(os.Stderr, "PCR: No name given — cancelled.")
			return nil
		}
		if err := store.RenameBundle(b.ID, newName); err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "PCR: Renamed %q → %q\n", b.Message, newName)
	case "d":
		confirm := strings.ToLower(strings.TrimSpace(ttyPrompt(tty, fmt.Sprintf("Delete bundle %q and return its prompts to drafts? [y/N] ", b.Message))))
		if confirm != "y" {
			fmt.Fprintln(os.Stderr, "PCR: Cancelled.")
			return nil
		}
		if err := store.DeleteBundle(b.ID); err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "PCR: Deleted bundle %q — prompts returned to drafts.\n", b.Message)
	case "s":
		if b.BundleStatus != "open" {
			fmt.Fprintln(os.Stderr, "PCR: Bundle is already sealed.")
			return nil
		}
		if err := store.CloseBundle(b.ID); err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "PCR: Sealed %q — push with `pcr push`\n", b.Message)
	default:
		fmt.Fprintln(os.Stderr, "PCR: Cancelled.")
	}
	return nil
}

// runRemoveFromBundle shows items in a bundle and lets the user remove selected ones.
func runRemoveFromBundle(bundleName string) error {
	bundle, err := store.GetBundleByName(bundleName)
	if err != nil {
		return err
	}
	if bundle == nil {
		return fmt.Errorf("no bundle named %q", bundleName)
	}
	full, err := store.GetCommitWithItems(bundle.ID)
	if err != nil {
		return err
	}
	if len(full.Items) == 0 {
		fmt.Fprintf(os.Stderr, "PCR: Bundle %q is empty.\n", bundleName)
		return nil
	}

	const dim = "\x1b[2m"
	const rst = "\x1b[0m"
	fmt.Fprintf(os.Stderr, "PCR: Bundle %q contains %d prompt%s:\n\n", bundleName, len(full.Items), plural(len(full.Items)))
	for i, d := range full.Items {
		date := formatCapturedAt(d.CapturedAt)
		fmt.Fprintf(os.Stderr, "  [%d] %s%s%s %q\n", i+1, dim, date, rst, truncate(d.PromptText, 60))
	}
	fmt.Fprintln(os.Stderr)

	tty := openTTY()
	defer tty.Close()

	resp := strings.TrimSpace(ttyPrompt(tty, "Select prompts to remove [e.g. 1,2 or all — enter to cancel]: "))
	if resp == "" {
		fmt.Fprintln(os.Stderr, "PCR: Nothing removed.")
		return nil
	}

	var selected []store.DraftRecord
	if strings.ToLower(resp) == "all" {
		selected = full.Items
	} else {
		selected = parseSelection(resp, full.Items)
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing removed.")
		return nil
	}

	if err := store.RemoveDraftsFromBundle(bundle.ID, draftIDs(selected)); err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Removed %d prompt%s from %q — they're back in drafts.\n",
		len(selected), plural(len(selected)), bundleName)
	return nil
}

// runDeleteDrafts shows unbundled drafts and permanently deletes the selection.
func runDeleteDrafts() error {
	ctx := resolveProjectContext()

	drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
	if err != nil {
		return err
	}
	staged, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	all := append(drafts, staged...)

	if len(all) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No draft prompts to delete.")
		return nil
	}

	const dim = "\x1b[2m"
	const rst = "\x1b[0m"
	fmt.Fprintf(os.Stderr, "PCR: %d draft prompt%s:\n\n", len(all), plural(len(all)))
	for idx, d := range all {
		date := formatCapturedAt(d.CapturedAt)
		fmt.Fprintf(os.Stderr, "  [%d] %s%s%s %q\n", idx+1, dim, date, rst, truncate(d.PromptText, 60))
	}
	fmt.Fprintln(os.Stderr)

	tty := openTTY()
	defer tty.Close()

	resp := strings.TrimSpace(ttyPrompt(tty, "Select prompts to delete [e.g. 1,2 or all — enter to cancel]: "))
	if resp == "" {
		fmt.Fprintln(os.Stderr, "PCR: Cancelled.")
		return nil
	}

	var selected []store.DraftRecord
	if strings.ToLower(resp) == "all" {
		selected = all
	} else {
		selected = parseSelection(resp, all)
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing deleted.")
		return nil
	}

	if err := store.DeleteDrafts(draftIDs(selected)); err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Deleted %d prompt%s.\n", len(selected), plural(len(selected)))
	return nil
}

func formatCapturedAt(s string) string {
	t, err := time.Parse(time.RFC3339, s)
	if err != nil {
		t, err = time.Parse("2006-01-02T15:04:05.999Z", s)
	}
	if err != nil {
		return ""
	}
	return t.Local().Format("Jan 2 15:04")
}

func init() {
	addCmd.Flags().Bool("clear", false, "Unstage all staged prompts")
	addCmd.Flags().Bool("remove", false, "Remove prompts from a bundle: pcr add --remove \"bundle-name\"")
	addCmd.Flags().Bool("delete", false, "Permanently delete draft prompts")
}
