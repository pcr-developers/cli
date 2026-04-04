package cmd

import (
	"fmt"
	"os"
	"time"

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
		// Only act if pcr start is actually running.
		if _, alive := readExistingPID(pidFilePath()); !alive {
			return nil
		}

		ctx := resolveProjectContext()

		// Poll up to 2s for the watcher to process new drafts — the Stop hook
		// fires immediately after the response, but the watcher has a 1s debounce.
		var drafts []store.DraftRecord
		var draftsErr error
		for i := 0; i < 4; i++ {
			drafts, draftsErr = store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
			if draftsErr != nil || len(drafts) > 0 {
				break
			}
			time.Sleep(500 * time.Millisecond)
		}
		if draftsErr != nil || len(drafts) == 0 {
			return nil
		}
		n := len(drafts)

		// Open /dev/tty — required because Claude Code holds stdin.
		ttyFile, err := os.OpenFile("/dev/tty", os.O_RDWR, 0600)
		if err != nil {
			return nil
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

		// Read keypresses, skipping mouse/escape sequences, until we get a
		// real letter or Enter. A mouse click sends \x1b[M... — we drain those
		// so the prompt doesn't vanish on an accidental click.
		var ch byte
		buf := make([]byte, 1)
		drain := make([]byte, 32)
		for {
			if _, err := ttyFile.Read(buf); err != nil {
				break
			}
			b := buf[0]
			if b == 0x1b { // ESC — start of escape/mouse sequence, drain and retry
				ttyFile.Read(drain) //nolint — best-effort flush
				continue
			}
			if b < 0x20 && b != '\r' && b != '\n' { // other control bytes, skip
				continue
			}
			ch = b
			break
		}

		if rawErr == nil {
			_ = term.Restore(fd, oldState)
		}
		_, _ = ttyFile.WriteString("\r\n")

		// Accept Y, y, or Enter (CR or LF) as confirmation; anything else = no.
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
