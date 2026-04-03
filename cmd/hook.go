package cmd

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"
	"golang.org/x/term"

	"github.com/pcr-developers/cli/internal/store"
)

// hookCmd is called by the Claude Code Stop hook after every response.
// It opens /dev/tty directly so it works even when Claude Code holds stdin.
// Always exits 0 — never re-engages the model.
var hookCmd = &cobra.Command{
	Use:    "hook",
	Short:  "Internal: called by Claude Code Stop hook",
	Hidden: true,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Only nudge if pcr start is actually running.
		if _, alive := readExistingPID(pidFilePath()); !alive {
			return nil
		}

		ctx := resolveProjectContext()

		drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
		if err != nil || len(drafts) == 0 {
			return nil // nothing to do
		}
		n := len(drafts)

		// Open /dev/tty — required because Claude Code holds stdin.
		ttyFile, err := os.OpenFile("/dev/tty", os.O_RDWR, 0600)
		if err != nil {
			return nil // non-interactive context, skip silently
		}
		defer ttyFile.Close()

		// Find the most recently touched open bundle, or fall back to git branch name.
		openBundles, _ := store.GetOpenBundles()
		var targetBundle *store.PromptCommit
		bundleName := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
		if bundleName == "" || bundleName == "HEAD" {
			bundleName = "untitled"
		}
		if len(openBundles) > 0 {
			targetBundle = &openBundles[0]
			bundleName = targetBundle.Message
		}

		msg := fmt.Sprintf("\r\nPCR: %d new prompt%s — add to %q? [Y/n] ", n, plural(n), bundleName)
		_, _ = ttyFile.WriteString(msg)

		// Switch to raw mode so we get a single keypress without requiring Enter.
		fd := int(ttyFile.Fd())
		oldState, rawErr := term.MakeRaw(fd)

		buf := make([]byte, 1)
		_, _ = ttyFile.Read(buf)

		if rawErr == nil {
			_ = term.Restore(fd, oldState)
		}
		_, _ = ttyFile.WriteString("\r\n")

		ch := buf[0]
		// Accept Y, y, or Enter (CR or LF) as confirmation.
		if ch != 'Y' && ch != 'y' && ch != '\r' && ch != '\n' {
			return nil
		}

		ids := draftIDs(drafts)

		if targetBundle != nil {
			if err := store.AddDraftsToBundle(targetBundle.ID, ids); err != nil {
				_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: error: %v\r\n", err))
				return nil
			}
			_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: Added %d prompt%s to %q\r\n", n, plural(n), bundleName))
		} else {
			projectID := ""
			projectName := ctx.name
			if len(ctx.ids) > 0 {
				projectID = ctx.ids[0]
			}
			syntheticSha := "hook-" + generateID()
			_, err := store.CreateCommit(bundleName, syntheticSha, ids, projectID, projectName, bundleName, "open")
			if err != nil {
				_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: error: %v\r\n", err))
				return nil
			}
			_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: Created bundle %q with %d prompt%s\r\n", bundleName, n, plural(n)))
		}

		return nil
	},
}
