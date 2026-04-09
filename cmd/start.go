package cmd

import (
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/config"
	"github.com/pcr-developers/cli/internal/display"
	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/sources"
)

var startVerbose bool

var startCmd = &cobra.Command{
	Use:   "start",
	Short: "Watch for new Claude Code and Cursor prompts and save them as drafts",
	RunE: func(cmd *cobra.Command, args []string) error {
		pidFile := pidFilePath()

		// Check for existing watcher
		if pid, alive := readExistingPID(pidFile); alive {
			if !isInteractiveTerminal() {
				// Non-interactive: always replace the existing watcher silently
				fmt.Fprintf(os.Stderr, "PCR: Replacing existing watcher (PID %d).\n", pid)
			} else {
				fmt.Fprintf(os.Stderr, "PCR: Watcher already running (PID %d). Replace it? [Y/n]: ", pid)
				tty := openTTY()
				answer := ""
				if tty != nil {
					answer = ttyPrompt(tty, "")
					tty.Close()
				}
				if strings.ToLower(strings.TrimSpace(answer)) == "n" {
					return nil
				}
			}
			if proc, err := os.FindProcess(pid); err == nil {
				_ = proc.Signal(syscall.SIGTERM)
			}
		}

		// Write our PID
		if err := os.MkdirAll(filepath.Dir(pidFile), 0755); err != nil {
			return err
		}
		if err := os.WriteFile(pidFile, []byte(strconv.Itoa(os.Getpid())), 0644); err != nil {
			return err
		}
		defer os.Remove(pidFile)

		a := auth.Load()
		userID := ""
		if a != nil {
			userID = a.UserID
		}

		display.Verbose = startVerbose

		projs := projects.Load()
		display.PrintStartupBanner(Version, BuildTime, len(projs))

		// Start all sources in goroutines
		allSources := sources.All()
		for _, src := range allSources {
			go src.Start(userID)
		}

		// Block until SIGINT/SIGTERM
		sigCh := make(chan os.Signal, 1)
		signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
		<-sigCh

		fmt.Fprintln(os.Stderr, "\nPCR: Watcher stopped.")
		return nil
	},
}

func init() {
	startCmd.Flags().BoolVarP(&startVerbose, "verbose", "v", false, "Print real-time events from all watchers (diffs, session state changes, completed turns)")
}

func pidFilePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "watcher.pid")
}

func readExistingPID(pidFile string) (int, bool) {
	data, err := os.ReadFile(pidFile)
	if err != nil {
		return 0, false
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		return 0, false
	}
	proc, err := os.FindProcess(pid)
	if err != nil {
		return 0, false
	}
	// Send signal 0 to check liveness
	err = proc.Signal(syscall.Signal(0))
	return pid, err == nil
}
