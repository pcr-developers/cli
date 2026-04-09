package claudecode

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/pcr-developers/cli/internal/store"
	"golang.org/x/term"
)

// RunHook is called by `pcr hook` after every Claude Code response. It finds
// any new drafts for the given project, prompts the user interactively via
// /dev/tty, and adds them to a bundle.
//
// projectIDs and projectName come from resolveProjectContext in cmd — the
// hook command owns context resolution; this function owns the hook logic.
func RunHook(projectIDs, projectNames []string, projectName string) error {
	// Poll up to 2s for the watcher to process new drafts — the Stop hook
	// fires immediately after the response, but the watcher has a 1s debounce.
	var drafts []store.DraftRecord
	var draftsErr error
	for i := 0; i < 4; i++ {
		drafts, draftsErr = store.GetDraftsByStatus(store.StatusDraft, projectIDs, projectNames)
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
	bundleName := gitBranch()
	if bundleName == "" || bundleName == "HEAD" {
		bundleName = "untitled"
	}
	if len(openBundles) > 0 {
		targetBundle = &openBundles[0]
		bundleName = targetBundle.Message
	}

	msg := fmt.Sprintf("\r\nPCR: %d new prompt%s — add to %q? [Y/n/b] ", n, plural(n), bundleName)
	_, _ = ttyFile.WriteString(msg)

	fd := int(ttyFile.Fd())
	oldState, rawErr := term.MakeRaw(fd)

	var ch byte
	buf := make([]byte, 1)
	drain := make([]byte, 32)
	for {
		if _, err := ttyFile.Read(buf); err != nil {
			break
		}
		b := buf[0]
		if b == 0x1b {
			ttyFile.Read(drain) //nolint — best-effort flush
			continue
		}
		if b < 0x20 && b != '\r' && b != '\n' {
			continue
		}
		ch = b
		break
	}

	if rawErr == nil {
		_ = term.Restore(fd, oldState)
	}
	_, _ = ttyFile.WriteString("\r\n")

	if ch == 'b' || ch == 'B' {
		return hookCreateNewBundle(ttyFile, drafts, projectIDs, projectName)
	}
	if ch == 'n' || ch == 'N' {
		return nil
	}
	if ch != 'Y' && ch != 'y' && ch != '\r' && ch != '\n' {
		return nil
	}

	return hookAddToBundle(ttyFile, drafts, targetBundle, bundleName, projectIDs, projectName, n)
}

func hookAddToBundle(ttyFile *os.File, drafts []store.DraftRecord, targetBundle *store.PromptCommit, bundleName string, projectIDs []string, projectName string, n int) error {
	ids := draftIDs(drafts)
	if targetBundle != nil {
		if err := store.AddDraftsToBundle(targetBundle.ID, ids); err != nil {
			_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: error: %v\r\n", err))
			return nil
		}
		_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: Added %d prompt%s to %q\r\n", n, plural(n), bundleName))
	} else {
		projectID := ""
		if len(projectIDs) > 0 {
			projectID = projectIDs[0]
		}
		syntheticSha := fmt.Sprintf("hook-%d", time.Now().UnixNano())
		_, err := store.CreateCommit(bundleName, syntheticSha, ids, projectID, projectName, bundleName, "open", false)
		if err != nil {
			_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: error: %v\r\n", err))
			return nil
		}
		_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: Created prompt bundle %q with %d prompt%s\r\n", bundleName, n, plural(n)))
	}
	return nil
}

func hookCreateNewBundle(ttyFile *os.File, drafts []store.DraftRecord, projectIDs []string, projectName string) error {
	_, _ = ttyFile.WriteString("PCR: New bundle name: ")

	reader := bufio.NewReader(ttyFile)
	line, _ := reader.ReadString('\n')
	name := strings.TrimSpace(strings.TrimRight(line, "\r\n"))

	if name == "" {
		_, _ = ttyFile.WriteString("PCR: Cancelled — no name given.\r\n")
		return nil
	}

	projectID := ""
	if len(projectIDs) > 0 {
		projectID = projectIDs[0]
	}

	ids := draftIDs(drafts)
	branch := gitBranch()
	syntheticSha := fmt.Sprintf("hook-%d", time.Now().UnixNano())
	_, err := store.CreateCommit(name, syntheticSha, ids, projectID, projectName, branch, "open", false)
	if err != nil {
		_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: error: %v\r\n", err))
		return nil
	}
	_, _ = ttyFile.WriteString(fmt.Sprintf("PCR: Created prompt bundle %q with %d prompt%s\r\n", name, len(drafts), plural(len(drafts))))
	return nil
}

func gitBranch() string {
	out, err := exec.Command("git", "rev-parse", "--abbrev-ref", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func draftIDs(drafts []store.DraftRecord) []string {
	ids := make([]string, len(drafts))
	for i, d := range drafts {
		ids[i] = d.ID
	}
	return ids
}

func plural(n int) string {
	if n == 1 {
		return ""
	}
	return "s"
}
