package cmd

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"strconv"
	"strings"

	"github.com/spf13/cobra"
	"golang.org/x/term"

	"github.com/pcr-developers/cli/internal/store"
)

func generateID() string {
	b := make([]byte, 8)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

var commitCmd = &cobra.Command{
	Use:   "commit [bundle-name]",
	Short: "Seal a bundle so it can be pushed",
	Long: `Seals a bundle, marking it ready to push. With no args, shows open bundles
to pick from or walks you through creating a new one from drafts.

  --rename "old" "new"  Rename any unpushed bundle

Examples:
  pcr commit "auth refactor"                      # seal the named bundle
  pcr commit                                      # interactive
  pcr commit --rename "auth refactor" "login fix" # rename a bundle`,
	RunE: func(cmd *cobra.Command, args []string) error {
		messageArg, _ := cmd.Flags().GetString("message")
		selectArg, _ := cmd.Flags().GetString("select")
		renameArg, _ := cmd.Flags().GetString("rename")

		// --rename "old name" "new name"  or  --rename "old name" with positional arg
		if renameArg != "" {
			newName := strings.TrimSpace(strings.Join(args, " "))
			if newName == "" {
				return fmt.Errorf("--rename requires a new name: pcr commit --rename \"old name\" \"new name\"")
			}
			return runRenameBundle(renameArg, newName)
		}

		// Resolve bundle name from positional arg or -m flag.
		bundleName := messageArg
		if bundleName == "" && len(args) > 0 {
			bundleName = strings.TrimSpace(strings.Join(args, " "))
		}

		if bundleName != "" {
			return runSealBundle(bundleName, selectArg)
		}

		// No name given: show open bundles or fall through to interactive create.
		return runInteractiveCommit(selectArg)
	},
}

// runSealBundle seals a named open bundle. If no open bundle with that name exists
// and staged drafts are present, creates a new sealed bundle.
func runSealBundle(name, selectArg string) error {
	bundle, err := store.GetOpenBundleByName(name)
	if err != nil {
		return err
	}
	if bundle != nil {
		if err := store.CloseBundle(bundle.ID); err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "PCR: Sealed bundle %q — push with `pcr push`\n", name)
		return nil
	}

	// No open bundle found — create + seal from staged/available drafts.
	ctx := resolveProjectContext()
	projectID := ""
	projectName := ctx.name
	if len(ctx.ids) > 0 {
		projectID = ctx.ids[0]
	}

	candidates, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	if len(candidates) == 0 {
		candidates, err = store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
		if err != nil {
			return err
		}
	}
	if len(candidates) == 0 {
		fmt.Fprintf(os.Stderr, "PCR: No open bundle named %q and no staged prompts. Use `pcr add %q` to build it first.\n", name, name)
		return nil
	}

	var selected []store.DraftRecord
	if selectArg != "" {
		selected = parseSelection(selectArg, candidates)
	} else {
		selected = candidates
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — skipped.")
		return nil
	}

	branch := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
	syntheticSha := "manual-" + generateID()
	_, err = store.CreateCommit(name, syntheticSha, draftIDs(selected), projectID, projectName, branch, "closed")
	if err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Bundled %d prompt%s as %q — push with `pcr push`\n",
		len(selected), plural(len(selected)), name)
	return nil
}

// runInteractiveCommit handles `pcr commit` with no bundle name.
func runInteractiveCommit(selectArg string) error {
	// Show open bundles first.
	openBundles, err := store.GetOpenBundles()
	if err != nil {
		return err
	}
	if len(openBundles) > 0 {
		fmt.Fprintf(os.Stderr, "PCR: %d open bundle%s:\n\n", len(openBundles), plural(len(openBundles)))
		for i, b := range openBundles {
			items, _ := store.GetCommitWithItems(b.ID)
			count := 0
			if items != nil {
				count = len(items.Items)
			}
			fmt.Fprintf(os.Stderr, "  [%d] %q  (%d prompt%s)\n", i+1, b.Message, count, plural(count))
		}
		fmt.Fprintln(os.Stderr)

		tty := openTTY()
		defer tty.Close()
		resp := ttyPrompt(tty, "Seal which bundle? [number or enter to create new]: ")
		resp = strings.TrimSpace(resp)
		if resp != "" {
			selected := parseSelectionIndices(resp, len(openBundles))
			if len(selected) > 0 {
				b := openBundles[selected[0]]
				if err := store.CloseBundle(b.ID); err != nil {
					return err
				}
				fmt.Fprintf(os.Stderr, "PCR: Sealed bundle %q — push with `pcr push`\n", b.Message)
				return nil
			}
		}
	}

	// Create new bundle from staged/available drafts.
	ctx := resolveProjectContext()
	projectID := ""
	projectName := ctx.name
	if len(ctx.ids) > 0 {
		projectID = ctx.ids[0]
	}

	candidates, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	usingStaged := len(candidates) > 0
	if !usingStaged {
		candidates, err = store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
		if err != nil {
			return err
		}
	}
	if len(candidates) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No prompts to bundle. Run `pcr add` or `pcr start` to capture prompts.")
		return nil
	}

	var selected []store.DraftRecord
	if selectArg != "" {
		selected = parseSelection(selectArg, candidates)
		if len(selected) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No valid selection — skipped.")
			return nil
		}
	} else if usingStaged {
		selected = candidates
		fmt.Fprintf(os.Stderr, "PCR: Bundling %d staged prompt%s\n", len(selected), plural(len(selected)))
		for i, d := range selected {
			fmt.Fprintf(os.Stderr, "  [%d] %q\n", i+1, truncate(d.PromptText, 72))
		}
		fmt.Fprintln(os.Stderr)
	} else {
		fmt.Fprintf(os.Stderr, "PCR: %d draft prompt%s available\n\n", len(candidates), plural(len(candidates)))
		for i, d := range candidates {
			fmt.Fprintf(os.Stderr, "  [%d] %q\n", i+1, truncate(d.PromptText, 72))
		}
		fmt.Fprintln(os.Stderr)
		tty := openTTY()
		defer tty.Close()
		resp := ttyPrompt(tty, "Select prompts to bundle [e.g. 1,2 or all]: ")
		resp = strings.TrimSpace(resp)
		if resp == "" || strings.ToLower(resp) == "none" {
			fmt.Fprintln(os.Stderr, "PCR: Skipped — no prompts bundled.")
			return nil
		}
		if strings.ToLower(resp) == "all" {
			selected = candidates
		} else {
			selected = parseSelection(resp, candidates)
		}
		if len(selected) == 0 {
			fmt.Fprintln(os.Stderr, "PCR: No valid selection — skipped.")
			return nil
		}
	}

	tty := openTTY()
	defer tty.Close()
	message := strings.TrimSpace(ttyPrompt(tty, "Bundle name: "))
	if message == "" {
		fmt.Fprintln(os.Stderr, "PCR: No name provided — skipped.")
		return nil
	}

	syntheticSha := "manual-" + generateID()
	_, err = store.CreateCommit(message, syntheticSha, draftIDs(selected), projectID, projectName, "", "closed")
	if err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Bundled %d prompt%s as %q — push with `pcr push`\n",
		len(selected), plural(len(selected)), message)
	return nil
}

func runRenameBundle(oldName, newName string) error {
	bundle, err := store.GetBundleByName(oldName)
	if err != nil {
		return err
	}
	if bundle == nil {
		return fmt.Errorf("no bundle named %q", oldName)
	}
	if err := store.RenameBundle(bundle.ID, newName); err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Renamed %q → %q\n", oldName, newName)
	return nil
}

func init() {
	commitCmd.Flags().String("select", "", "Non-interactive selection (e.g. 1,2,3)")
	commitCmd.Flags().StringP("message", "m", "", "Bundle name to seal or create")
	commitCmd.Flags().String("rename", "", "Rename a bundle: pcr commit --rename \"old name\" \"new name\"")
}

// ─── TTY helpers ─────────────────────────────────────────────────────────────

type ttyHandle struct {
	f      *os.File
	reader *bufio.Reader
}

func (t *ttyHandle) Close() {
	if t.f != nil {
		_ = t.f.Close()
	}
}

func openTTY() *ttyHandle {
	f, err := os.OpenFile("/dev/tty", os.O_RDWR, 0600)
	if err != nil {
		return &ttyHandle{reader: bufio.NewReader(os.Stdin)}
	}
	return &ttyHandle{f: f, reader: bufio.NewReader(f)}
}

func ttyPrompt(tty *ttyHandle, question string) string {
	if tty.f != nil {
		_, _ = tty.f.WriteString(question)
	} else {
		fmt.Fprint(os.Stderr, question)
	}
	line, _ := tty.reader.ReadString('\n')
	return strings.TrimRight(line, "\r\n")
}

func hasTTY() bool {
	return term.IsTerminal(int(os.Stdin.Fd()))
}

// ─── Selection helpers ────────────────────────────────────────────────────────

func parseSelectionIndices(input string, max int) []int {
	seen := map[int]bool{}
	var result []int
	for _, part := range strings.Split(input, ",") {
		t := strings.TrimSpace(part)
		if idx := strings.Index(t, "-"); idx > 0 {
			from, _ := strconv.Atoi(t[:idx])
			to, _ := strconv.Atoi(t[idx+1:])
			for i := from - 1; i <= to-1; i++ {
				if i >= 0 && i < max && !seen[i] {
					result = append(result, i)
					seen[i] = true
				}
			}
		} else {
			n, _ := strconv.Atoi(t)
			i := n - 1
			if i >= 0 && i < max && !seen[i] {
				result = append(result, i)
				seen[i] = true
			}
		}
	}
	return result
}

func parseSelection(input string, all []store.DraftRecord) []store.DraftRecord {
	indices := parseSelectionIndices(input, len(all))
	result := make([]store.DraftRecord, 0, len(indices))
	for _, i := range indices {
		result = append(result, all[i])
	}
	return result
}

func draftIDs(drafts []store.DraftRecord) []string {
	ids := make([]string, len(drafts))
	for i, d := range drafts {
		ids[i] = d.ID
	}
	return ids
}

func truncate(text string, n int) string {
	flat := strings.Join(strings.Fields(text), " ")
	if len(flat) > n {
		return flat[:n-1] + "…"
	}
	return flat
}

func plural(n int) string {
	if n == 1 {
		return ""
	}
	return "s"
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
