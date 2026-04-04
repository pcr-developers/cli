package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/store"
)

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "Show auth, registered projects, and prompt bundle state",
	Run: func(cmd *cobra.Command, args []string) {
		const bold = "\x1b[1m"
		const dim = "\x1b[2m"
		const grn = "\x1b[32m"
		const yel = "\x1b[33m"
		const rst = "\x1b[0m"

		// ── Auth ──────────────────────────────────────────────────────────────
		a := auth.Load()
		if a != nil {
			fmt.Fprintf(os.Stderr, "%s✓%s Logged in (user: %s)\n", grn, rst, a.UserID)
		} else {
			fmt.Fprintln(os.Stderr, "Not logged in — run `pcr login`")
		}

		// ── Projects ──────────────────────────────────────────────────────────
		projs := projects.Load()
		fmt.Fprintln(os.Stderr)
		if len(projs) == 0 {
			fmt.Fprintln(os.Stderr, "No projects registered. Run `pcr init` in a project directory.")
		} else {
			fmt.Fprintf(os.Stderr, "%sProjects%s\n", bold, rst)
			for _, p := range projs {
				remote := ""
				if p.ProjectID != "" {
					remote = fmt.Sprintf("  %s[remote: %s]%s", dim, p.ProjectID, rst)
				}
				fmt.Fprintf(os.Stderr, "  %s%s\n  %s%s%s\n", p.Name, remote, dim, p.Path, rst)
			}
		}

		// ── Bundles ───────────────────────────────────────────────────────────
		unpushed, err := store.GetUnpushedCommits()
		if err == nil {
			fmt.Fprintln(os.Stderr)
			if len(unpushed) == 0 {
				fmt.Fprintf(os.Stderr, "%sBundles%s  none — everything pushed\n", bold, rst)
			} else {
				fmt.Fprintf(os.Stderr, "%sBundles%s\n", bold, rst)
				for _, b := range unpushed {
					full, _ := store.GetCommitWithItems(b.ID)
					count := 0
					if full != nil {
						count = len(full.Items)
					}
					if b.BundleStatus == "open" {
						fmt.Fprintf(os.Stderr, "  %s●%s  %s  %s(%d prompt%s)%s\n",
							yel, rst, b.Message, dim, count, plural(count), rst)
					} else {
						fmt.Fprintf(os.Stderr, "  %s✓%s  %s  %s(%d prompt%s — sealed, ready to push)%s\n",
							grn, rst, b.Message, dim, count, plural(count), rst)
					}
				}
			}
		}

		// ── Drafts ────────────────────────────────────────────────────────────
		drafts, err := store.GetDraftsByStatus(store.StatusDraft, nil, nil)
		if err == nil {
			fmt.Fprintln(os.Stderr)
			if len(drafts) == 0 {
				fmt.Fprintf(os.Stderr, "%sDrafts%s  none\n", bold, rst)
			} else {
				fmt.Fprintf(os.Stderr, "%sDrafts%s  %d unreviewed — run `pcr bundle` to create a prompt bundle\n", bold, rst, len(drafts))
			}
		}
	},
}
