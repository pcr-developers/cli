package cmd

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"

	"golang.org/x/term"

	"github.com/pcr-developers/cli/internal/store"
)

func generateID() string {
	b := make([]byte, 8)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
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

// isInteractiveTerminal returns true only in a real interactive terminal.
// The agent shell (CURSOR_AGENT=1, TERM=dumb) and CI/pipe environments are
// non-interactive. Cursor's integrated terminal tabs (TERM_PROGRAM=vscode)
// ARE interactive — they have a real pty — but we use stdin instead of
// /dev/tty there (see openTTY).
func isInteractiveTerminal() bool {
	if os.Getenv("CURSOR_AGENT") == "1" {
		return false
	}
	if os.Getenv("TERM") == "dumb" {
		return false
	}
	if os.Getenv("CURSOR_SANDBOX") != "" {
		return false
	}
	return term.IsTerminal(int(os.Stdin.Fd()))
}

var (
	stdinReader     *bufio.Reader
	stdinReaderOnce sync.Once
)

func sharedStdinReader() *bufio.Reader {
	stdinReaderOnce.Do(func() {
		stdinReader = bufio.NewReader(os.Stdin)
	})
	return stdinReader
}

func openTTY() *ttyHandle {
	if !isInteractiveTerminal() {
		return nil
	}
	if os.Getenv("TERM_PROGRAM") == "vscode" {
		return &ttyHandle{reader: sharedStdinReader()}
	}
	f, err := os.OpenFile("/dev/tty", os.O_RDWR, 0600)
	if err != nil {
		return &ttyHandle{reader: sharedStdinReader()}
	}
	return &ttyHandle{f: f, reader: bufio.NewReader(f)}
}

// openHookTTY opens /dev/tty directly for the Stop hook command.
// The hook runs inside a tool's process that holds stdin, so it always
// needs /dev/tty regardless of the terminal environment.
func openHookTTY() *ttyHandle {
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
	return isInteractiveTerminal()
}

func nonInteractiveHint(cmd string) {
	fmt.Fprintln(os.Stderr, "PCR: Interactive mode not available in this terminal.")
	fmt.Fprintf(os.Stderr, "     Use flags: %s\n", cmd)
	fmt.Fprintln(os.Stderr, "     Run from Terminal.app or iTerm2 for the full interactive experience.")
}

// ─── Selection helpers ────────────────────────────────────────────────────────

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

func parseSelectionIndices(input string, max int) []int {
	seen := map[int]bool{}
	var result []int
	for _, part := range strings.Split(input, ",") {
		t := strings.TrimSpace(part)
		if idx := strings.Index(t, "-"); idx > 0 {
			from, errA := strconv.Atoi(t[:idx])
			to, errB := strconv.Atoi(t[idx+1:])
			if errA != nil || errB != nil {
				fmt.Fprintf(os.Stderr, "PCR: Invalid selection %q — use numbers only (e.g. 1-4, 2,5,7, or all)\n", t)
				continue
			}
			if from > to {
				fmt.Fprintf(os.Stderr, "PCR: Invalid range %q — use low-to-high order (e.g. %d-%d not %d-%d)\n", t, to, from, from, to)
				continue
			}
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

// ─── Text helpers ─────────────────────────────────────────────────────────────

func truncate(text string, n int) string {
	flat := strings.Join(strings.Fields(text), " ")
	if len(flat) > n {
		return flat[:n-1] + "…"
	}
	return flat
}

// promptPreview returns a useful preview of a prompt for display in list views.
// It strips leading @file references and shows the last meaningful instruction line.
func promptPreview(text string, n int) string {
	lines := strings.Split(strings.TrimSpace(text), "\n")
	var meaningful []string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if strings.HasPrefix(line, "@/") || strings.HasPrefix(line, "@~") {
			continue
		}
		if strings.HasPrefix(line, "/Users/") || strings.HasPrefix(line, "/home/") {
			continue
		}
		meaningful = append(meaningful, line)
	}
	if len(meaningful) == 0 {
		return truncate(text, n)
	}
	preview := meaningful[len(meaningful)-1]
	if len(meaningful) > 1 && len(meaningful[0]) <= n/2 {
		preview = meaningful[0]
	}
	return truncate(preview, n)
}

func filterNonEmpty(lines []string) []string {
	var result []string
	for _, l := range lines {
		if strings.TrimSpace(l) != "" {
			result = append(result, l)
		}
	}
	return result
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
