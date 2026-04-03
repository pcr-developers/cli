package cmd

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/auth"
	"github.com/pcr-developers/cli/internal/config"
	"github.com/pcr-developers/cli/internal/supabase"
)

var loginCmd = &cobra.Command{
	Use:   "login",
	Short: "Authenticate with PCR.dev",
	RunE: func(cmd *cobra.Command, args []string) error {
		settingsURL := config.AppURL + "/settings"
		fmt.Fprintf(os.Stderr, "\nPCR: Opening %s to get your CLI token...\n", settingsURL)
		openBrowser(settingsURL)

		fmt.Fprint(os.Stderr, "Paste your CLI token: ")
		reader := bufio.NewReader(os.Stdin)
		token, err := reader.ReadString('\n')
		if err != nil {
			return fmt.Errorf("failed to read token: %w", err)
		}
		token = strings.TrimSpace(token)
		if token == "" {
			return fmt.Errorf("no token provided")
		}

		fmt.Fprintln(os.Stderr, "PCR: Validating token...")
		userID, err := supabase.ValidateCLIToken(token)
		if err != nil || userID == "" {
			return fmt.Errorf("invalid token — please check your token at %s", settingsURL)
		}

		if err := auth.Save(&auth.Auth{Token: token, UserID: userID}); err != nil {
			return fmt.Errorf("failed to save credentials: %w", err)
		}

		fmt.Fprintf(os.Stderr, "PCR: Logged in successfully (user: %s)\n\n", userID)
		return nil
	},
}

func openBrowser(url string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", url)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	default:
		cmd = exec.Command("xdg-open", url)
	}
	_ = cmd.Start()
}
