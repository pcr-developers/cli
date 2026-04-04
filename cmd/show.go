package cmd

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/store"
)

// shortFilePath trims an absolute file path to the shortest useful form.
// For files inside a registered project, returns "repo-name/sub/path".
// For other absolute paths, returns the last 3 path components.
func shortFilePath(path string, projByID map[string]string) string {
	// Try to match against each registered project path.
	for _, p := range projects.Load() {
		if p.Path == "" {
			continue
		}
		prefix := p.Path + "/"
		if strings.HasPrefix(path, prefix) {
			rel := path[len(prefix):]
			return p.Name + "/" + rel
		}
	}
	// Fall back: last 3 components of the path.
	parts := strings.Split(filepath.Clean(path), string(filepath.Separator))
	if len(parts) > 3 {
		parts = parts[len(parts)-3:]
	}
	return strings.Join(parts, "/")
}

var showCmd = &cobra.Command{
	Use:   "show <number>",
	Short: "Show the full content of a draft prompt by its list number",
	Long: `Shows the complete prompt text, response, model, timestamp, and metadata
for a specific draft. Use the number shown by pcr add or pcr status.

Examples:
  pcr show 22     # show draft #22 in full
  pcr show 1      # show the first draft`,
	Args: cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		n, err := strconv.Atoi(strings.TrimSpace(args[0]))
		if err != nil || n < 1 {
			return fmt.Errorf("invalid number %q — use the number from `pcr add` or `pcr status`", args[0])
		}

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
			fmt.Fprintln(os.Stderr, "PCR: No draft prompts.")
			return nil
		}
		if n > len(all) {
			return fmt.Errorf("draft #%d doesn't exist — you have %d draft%s (1–%d)", n, len(all), plural(len(all)), len(all))
		}

		d := all[n-1]

		const (
			bold = "\x1b[1m"
			dim  = "\x1b[2m"
			cyan = "\x1b[36m"
			grn  = "\x1b[32m"
			gry  = "\x1b[90m"
			rst  = "\x1b[0m"
		)

		// Build a project ID→name map from the local registry for repo lookups.
		projByID := map[string]string{}
		for _, p := range projects.Load() {
			if p.ProjectID != "" {
				projByID[p.ProjectID] = p.Name
			}
		}
		repoName := func(id string) string {
			if name, ok := projByID[id]; ok {
				return name
			}
			return id // fall back to raw ID if not registered locally
		}

		fmt.Fprintf(os.Stderr, "\n%s[%d] Draft prompt%s\n", bold, n, rst)
		fmt.Fprintf(os.Stderr, "%s─────────────────────────────────────────%s\n", gry, rst)

		// Metadata line: time · source · model · branch
		meta := []string{}
		if d.CapturedAt != "" {
			meta = append(meta, fmtTime(d.CapturedAt))
		}
		if d.Source != "" {
			meta = append(meta, d.Source)
		}
		if d.Model != "" {
			meta = append(meta, d.Model)
		}
		if d.BranchName != "" {
			meta = append(meta, "branch:"+d.BranchName)
		}
		if len(meta) > 0 {
			fmt.Fprintf(os.Stderr, "%s%s%s\n", dim, strings.Join(meta, "  ·  "), rst)
		}

		// Repo attribution
		touchedIDs := d.TouchedProjectIDs()
		if len(touchedIDs) > 1 {
			// Cross-repo prompt — show all touched repos.
			names := make([]string, 0, len(touchedIDs))
			for _, id := range touchedIDs {
				names = append(names, repoName(id))
			}
			fmt.Fprintf(os.Stderr, "%srepos: %s%s\n", dim, strings.Join(names, ", "), rst)
		} else if d.ProjectName != "" {
			fmt.Fprintf(os.Stderr, "%srepo:  %s%s\n", dim, d.ProjectName, rst)
		}
		fmt.Fprintln(os.Stderr)

		// Full prompt text
		fmt.Fprintf(os.Stderr, "%s%sPROMPT%s\n", bold, cyan, rst)
		fmt.Fprintf(os.Stderr, "%s\n", d.PromptText)

		// Response if present
		if d.ResponseText != "" {
			fmt.Fprintf(os.Stderr, "\n%s%sRESPONSE%s\n", bold, grn, rst)
			resp := d.ResponseText
			if len(resp) > 1000 {
				resp = resp[:1000] + fmt.Sprintf("\n%s… (%d more chars)%s", dim, len(d.ResponseText)-1000, rst)
			}
			fmt.Fprintf(os.Stderr, "%s\n", resp)
		}

		// Relevant files — show short paths (relative to project root when possible).
		if fc := d.FileContext; fc != nil {
			if files, ok := fc["relevant_files"]; ok {
				if fileList, ok := files.([]any); ok && len(fileList) > 0 {
					fmt.Fprintf(os.Stderr, "\n%s%sFILES IN CONTEXT%s\n", bold, gry, rst)
					for _, f := range fileList {
						path := fmt.Sprintf("%v", f)
						// Show the shortest useful suffix: repo/subpath
						short := shortFilePath(path, projByID)
						fmt.Fprintf(os.Stderr, "  %s%s%s\n", dim, short, rst)
					}
				}
			}
		}

		fmt.Fprintln(os.Stderr)
		return nil
	},
}
