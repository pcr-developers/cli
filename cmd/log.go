package cmd

import (
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/store"
)

var logCmd = &cobra.Command{
	Use:     "log",
	Aliases: []string{"review"},
	Short:   "Show local prompt state (pushed/committed/draft)",
	RunE: func(cmd *cobra.Command, args []string) error {
		const (
			grn  = "\x1b[32m"
			ylw  = "\x1b[33m"
			gry  = "\x1b[90m"
			cyan = "\x1b[36m"
			bold = "\x1b[1m"
			dim  = "\x1b[2m"
			rst  = "\x1b[0m"
		)

		// Detect current project context (current dir + ancestor projects)
		ctx := resolveProjectContext()

		// Pushed commits
		pushed := true
		pushedCommits, err := store.ListCommits(&pushed, ctx.ids, ctx.names)
		if err != nil {
			return err
		}

		// Unpushed committed
		unpushed := false
		unpushedCommits, err := store.ListCommits(&unpushed, ctx.ids, ctx.names)
		if err != nil {
			return err
		}

		// Drafts
		drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
		if err != nil {
			return err
		}

		// Show project context header
		branch := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
		if ctx.name != "" {
			if branch != "" && branch != "HEAD" {
				fmt.Fprintf(os.Stderr, "\n%s%s  %s  %s(%s)%s", bold, cyan, ctx.name, gry, branch, rst)
			} else {
				fmt.Fprintf(os.Stderr, "\n%s%s  %s%s", bold, cyan, ctx.name, rst)
			}
			if len(ctx.ids) == 0 {
				fmt.Fprintf(os.Stderr, "  %s(run `pcr init` to link remotely)%s", gry, rst)
			}
			fmt.Fprintln(os.Stderr)
		}

		if len(pushedCommits) == 0 && len(unpushedCommits) == 0 && len(drafts) == 0 {
			if ctx.name == "" {
				fmt.Fprintln(os.Stderr, "PCR: Nothing to show. Run `pcr init` in your project directory first.")
			} else {
				fmt.Fprintln(os.Stderr, "PCR: Nothing to show for this project. Run `pcr start` to capture prompts.")
			}
			return nil
		}

		if len(pushedCommits) > 0 {
			fmt.Fprintf(os.Stderr, "\n%s%s  PUSHED%s  (%d)\n", grn, bold, rst, len(pushedCommits))
			for _, c := range pushedCommits {
				fmt.Fprintf(os.Stderr, "  %s✓%s %s  %s%s%s\n",
					grn, rst,
					shortSha(c.HeadSha),
					bold, c.Message, rst)
				fmt.Fprintf(os.Stderr, "    %s%s%s\n", gry, fmtTime(c.PushedAt), rst)
			}
		}

		if len(unpushedCommits) > 0 {
			fmt.Fprintf(os.Stderr, "\n%s%s  COMMITTED — not yet pushed%s  (%d)\n", ylw, bold, rst, len(unpushedCommits))
			for _, c := range unpushedCommits {
				items, _ := store.GetCommitWithItems(c.ID)
				count := 0
				if items != nil {
					count = len(items.Items)
				}
				fmt.Fprintf(os.Stderr, "  %s⊙%s %s  %s%s%s  %s(%d prompt%s)%s\n",
					ylw, rst,
					shortSha(c.HeadSha),
					bold, c.Message, rst,
					gry, count, plural(count), rst)
			}
			fmt.Fprintf(os.Stderr, "\n  %sRun `pcr push` to upload these to PCR.dev%s\n", gry, rst)
		}

		if len(drafts) > 0 {
			fmt.Fprintf(os.Stderr, "\n%s%s  DRAFTS — not yet bundled%s  (%d)\n", cyan, bold, rst, len(drafts))
			for _, d := range drafts {
				preview := truncate(d.PromptText, 65)
				fmt.Fprintf(os.Stderr, "  %s◦%s %s%s%s  %s%s%s\n",
					gry, rst,
					bold, preview, rst,
					dim, strings.Split(d.CapturedAt, "T")[0], rst)
			}
			fmt.Fprintf(os.Stderr, "\n  %sRun `pcr add` then `pcr commit -m \"message\"` to bundle%s\n", gry, rst)
		}

		fmt.Fprintln(os.Stderr)
		return nil
	},
}

func shortSha(sha string) string {
	if strings.HasPrefix(sha, "manual-") {
		return "[manual]"
	}
	if len(sha) >= 7 {
		return sha[:7]
	}
	return sha
}

func fmtTime(iso string) string {
	if iso == "" {
		return ""
	}
	t, err := time.Parse(time.RFC3339, iso)
	if err != nil {
		return iso
	}
	return t.Format("2006-01-02 15:04")
}
