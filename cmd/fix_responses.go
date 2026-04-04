package cmd

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/sources/claudecode"
	"github.com/pcr-developers/cli/internal/store"
)

var fixResponsesCmd = &cobra.Command{
	Use:   "fix-responses",
	Short: "Backfill response_text for drafts captured without Claude's response",
	RunE: func(cmd *cobra.Command, args []string) error {
		repush, _ := cmd.Flags().GetBool("repush")

		home, _ := os.UserHomeDir()
		claudeProjectsDir := filepath.Join(home, ".claude", "projects")

		// Walk all Claude project directories, match each JSONL to a registered project
		// (same ancestor-matching logic the watcher uses), and backfill response_text.
		// Group prompts by session_id for the fuzzy updater.
		type sessionEntry struct {
			prompts map[string]string // promptText → responseText
		}
		sessions := map[string]*sessionEntry{}

		projDirs, err := os.ReadDir(claudeProjectsDir)
		if err != nil {
			return fmt.Errorf("cannot read %s: %w", claudeProjectsDir, err)
		}

		for _, projDir := range projDirs {
			if !projDir.IsDir() {
				continue
			}
			slug := projDir.Name()
			project := projects.GetProjectForClaudeSlug(slug)
			if project == nil {
				continue
			}

			sessionDir := filepath.Join(claudeProjectsDir, slug)
			entries, err := os.ReadDir(sessionDir)
			if err != nil {
				continue
			}

			for _, entry := range entries {
				if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".jsonl") {
					continue
				}
				filePath := filepath.Join(sessionDir, entry.Name())
				content, err := os.ReadFile(filePath)
				if err != nil {
					continue
				}

				session := claudecode.ParseClaudeCodeSession(string(content), project.Name, filePath)
				for _, p := range session.Prompts {
					if p.ResponseText == "" {
						continue
					}
					e, ok := sessions[p.SessionID]
					if !ok {
						e = &sessionEntry{prompts: map[string]string{}}
						sessions[p.SessionID] = e
					}
					e.prompts[p.PromptText] = p.ResponseText
				}
			}
		}

		updated := 0
		for sessionID, e := range sessions {
			n, err := store.UpdateDraftResponseFuzzy(sessionID, e.prompts)
			if err != nil {
				fmt.Fprintf(os.Stderr, "error updating session %s: %v\n", sessionID, err)
			}
			updated += n
		}

		fmt.Printf("Updated response text for %d drafts.\n", updated)

		if repush {
			pushed, err := store.ListPushedCommits()
			if err != nil {
				return fmt.Errorf("could not list pushed bundles: %w", err)
			}
			if len(pushed) == 0 {
				fmt.Println("No pushed bundles to reset.")
				return nil
			}
			for _, c := range pushed {
				if err := store.UnmarkPushed(c.ID); err != nil {
					fmt.Fprintf(os.Stderr, "error resetting bundle %q: %v\n", c.Message, err)
				} else {
					fmt.Printf("Reset bundle %q — run `pcr push` to re-push with updated responses.\n", c.Message)
				}
			}
		} else {
			fmt.Println("Run with --repush to reset pushed bundles so `pcr push` re-sends them.")
		}

		return nil
	},
}

func init() {
	fixResponsesCmd.Flags().Bool("repush", false, "Reset pushed bundles so `pcr push` re-sends them with updated response text")
	rootCmd.AddCommand(fixResponsesCmd)
}
