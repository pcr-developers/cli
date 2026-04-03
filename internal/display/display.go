package display

import (
	"fmt"
	"os"
	"strings"
	"time"
)

// All output goes to stderr so it never interferes with MCP stdio.

const (
	reset  = "\x1b[0m"
	bold   = "\x1b[1m"
	dim    = "\x1b[2m"
	cyan   = "\x1b[36m"
	green  = "\x1b[32m"
	yellow = "\x1b[33m"
	magenta = "\x1b[35m"
	gray   = "\x1b[90m"
)

func c(code, text string) string {
	return code + text + reset
}

func timestamp() string {
	return time.Now().Format("15:04:05")
}

func PrintStartupBanner(version string, projectCount int) {
	w := 56
	line := strings.Repeat("─", w)
	fmt.Fprintf(os.Stderr, "\n%s %s — live capture stream\n",
		c(cyan+bold, "PCR.dev"), c(gray, "v"+version))
	fmt.Fprintln(os.Stderr, c(gray, line))
	if projectCount == 0 {
		fmt.Fprintf(os.Stderr, "%s%s\n",
			c(yellow, "  ⚠  No projects registered."),
			c(gray, " Run `pcr init` in a project directory."))
	} else {
		plural := ""
		if projectCount != 1 {
			plural = "s"
		}
		fmt.Fprintf(os.Stderr, "%s\n",
			c(gray, fmt.Sprintf("  Watching %d project%s — new exchanges appear below as they happen.", projectCount, plural)))
	}
	fmt.Fprintln(os.Stderr, c(gray, line))
	fmt.Fprintln(os.Stderr)
}

type CaptureDisplayOptions struct {
	ProjectName  string
	SessionID    string
	Branch       string
	Model        string
	PromptText   string
	ToolCalls    []map[string]any
	InputTokens  int
	OutputTokens int
	ExchangeCount int
	ProjectURL   string
}

func SummarizeTools(toolCalls []map[string]any) string {
	counts := map[string]int{}
	for _, tc := range toolCalls {
		name, _ := tc["tool"].(string)
		if name == "" {
			name = "unknown"
		}
		counts[name]++
	}
	parts := []string{}
	for name, count := range counts {
		if count > 1 {
			parts = append(parts, fmt.Sprintf("%s×%d", name, count))
		} else {
			parts = append(parts, name)
		}
	}
	return strings.Join(parts, "  ")
}

func PrintCaptured(opts CaptureDisplayOptions) {
	ts := timestamp()
	branchStr := ""
	if opts.Branch != "" {
		branchStr = c(gray, " ["+opts.Branch+"]")
	}
	modelStr := ""
	if opts.Model != "" {
		modelStr = c(gray, "  "+opts.Model)
	}
	header := fmt.Sprintf("  %s%s%s  %s", c(bold, opts.ProjectName), branchStr, modelStr, c(gray, ts))

	preview := opts.PromptText
	if len(preview) > 80 {
		preview = strings.TrimRight(preview[:77], " ") + "…"
	}
	promptLine := fmt.Sprintf("  %s %s\n", c(cyan, "❯"), c(bold, `"`+preview+`"`))

	toolLine := ""
	if len(opts.ToolCalls) > 0 {
		toolLine = fmt.Sprintf("    %s\n", c(magenta, SummarizeTools(opts.ToolCalls)))
	}

	tokenLine := ""
	if opts.InputTokens > 0 || opts.OutputTokens > 0 {
		parts := []string{}
		if opts.InputTokens > 0 {
			parts = append(parts, fmt.Sprintf("%d in", opts.InputTokens))
		}
		if opts.OutputTokens > 0 {
			parts = append(parts, fmt.Sprintf("%d out", opts.OutputTokens))
		}
		tokenLine = fmt.Sprintf("    %s\n", c(gray, "tokens: "+strings.Join(parts, " · ")))
	}

	syncMsg := "1 exchange synced"
	if opts.ExchangeCount != 1 {
		syncMsg = fmt.Sprintf("%d exchanges synced", opts.ExchangeCount)
	}
	urlPart := ""
	if opts.ProjectURL != "" {
		urlPart = c(gray, "  →  "+opts.ProjectURL)
	}
	syncLine := fmt.Sprintf("  %s %s%s\n", c(green, "✓"), c(gray, syncMsg), urlPart)

	fmt.Fprintf(os.Stderr, "%s\n%s%s%s%s\n", header, promptLine, toolLine, tokenLine, syncLine)
}

type DraftDisplayOptions struct {
	ProjectName   string
	Branch        string
	PromptText    string
	ExchangeCount int
}

// PrintDrafted is used when prompts are saved locally (no Supabase sync).
// Shows a yellow ◎ indicator instead of the green ✓ sync line.
func PrintDrafted(opts DraftDisplayOptions) {
	ts := timestamp()
	branchStr := ""
	if opts.Branch != "" {
		branchStr = c(gray, " ["+opts.Branch+"]")
	}
	header := fmt.Sprintf("  %s%s  %s", c(bold, opts.ProjectName), branchStr, c(gray, ts))

	preview := opts.PromptText
	if len(preview) > 80 {
		preview = strings.TrimRight(preview[:77], " ") + "…"
	}
	promptLine := fmt.Sprintf("  %s %s\n", c(cyan, "❯"), c(bold, `"`+preview+`"`))

	count := "1 exchange"
	if opts.ExchangeCount != 1 {
		count = fmt.Sprintf("%d exchanges", opts.ExchangeCount)
	}
	draftLine := fmt.Sprintf("  %s %s\n", c(yellow, "◎"), c(gray, count+" saved locally — will surface on next git commit"))

	fmt.Fprintf(os.Stderr, "%s\n%s%s\n", header, promptLine, draftLine)
}

func PrintWatcherReady(sourceName, dir string) {
	fmt.Fprintf(os.Stderr, "  %s  %s\n", c(gray, "◎  "+sourceName), c(dim, dir))
}

func PrintError(context, msg string) {
	fmt.Fprintf(os.Stderr, "  %s %s\n", c(yellow, "⚠  "+context+":"), msg)
}

func Println(msg string) {
	fmt.Fprintln(os.Stderr, msg)
}

func Printf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format, args...)
}
