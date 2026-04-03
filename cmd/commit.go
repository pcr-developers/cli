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
	Use:   "commit",
	Short: "Bundle draft prompts into a named bundle",
	RunE: func(cmd *cobra.Command, args []string) error {
		messageArg, _ := cmd.Flags().GetString("message")
		selectArg, _ := cmd.Flags().GetString("select")

		ctx := resolveProjectContext()
		projectID := ""
		projectName := ctx.name
		if len(ctx.ids) > 0 {
			projectID = ctx.ids[0]
		}

		// Prefer staged drafts; fall back to all drafts if nothing is staged.
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
			fmt.Fprintln(os.Stderr, "PCR: No prompts to bundle. Run `pcr add` to stage prompts, or `pcr start` to capture new ones.")
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
			// Use all staged drafts without prompting.
			selected = candidates
			fmt.Fprintf(os.Stderr, "PCR: Bundling %d staged prompt%s\n", len(selected), plural(len(selected)))
			for i, d := range selected {
				fmt.Fprintf(os.Stderr, "  [%d] %q\n", i+1, truncate(d.PromptText, 72))
			}
			fmt.Fprintln(os.Stderr)
		} else {
			// Interactive selection from all drafts.
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

		// Get or prompt for message.
		message := messageArg
		if message == "" {
			tty := openTTY()
			defer tty.Close()
			message = strings.TrimSpace(ttyPrompt(tty, "Bundle message: "))
			if message == "" {
				fmt.Fprintln(os.Stderr, "PCR: No message provided — skipped.")
				return nil
			}
		}

		syntheticSha := "manual-" + generateID()

		_, err = store.CreateCommit(message, syntheticSha, draftIDs(selected), projectID, projectName, "")
		if err != nil {
			return err
		}

		fmt.Fprintf(os.Stderr, "PCR: bundled %d prompt%s — push with `pcr push`\n",
			len(selected), plural(len(selected)))
		return nil
	},
}

func init() {
	commitCmd.Flags().String("select", "", "Non-interactive selection (e.g. 1,2,3)")
	commitCmd.Flags().StringP("message", "m", "", "Bundle message")
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
