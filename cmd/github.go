package cmd

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/config"
)

var githubCmd = &cobra.Command{
	Use:   "github [setup|status]",
	Short: "Set up GitHub PR integration",
	RunE: func(cmd *cobra.Command, args []string) error {
		sub := "status"
		if len(args) > 0 {
			sub = args[0]
		}
		switch sub {
		case "setup":
			return githubSetup()
		case "status":
			return githubStatus()
		default:
			fmt.Fprint(os.Stderr, `
pcr github — GitHub PR integration

Usage:
  pcr github setup    Set up the webhook (deploys Edge Function, creates webhook)
  pcr github status   Show current configuration and webhook URL
`)
			return nil
		}
	},
}

// ─── config file ─────────────────────────────────────────────────────────────

type githubConfig struct {
	WebhookSecret string `json:"webhookSecret"`
	ConfiguredAt  string `json:"configuredAt"`
}

func githubConfigPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "github.json")
}

func loadGithubConfig() *githubConfig {
	data, err := os.ReadFile(githubConfigPath())
	if err != nil {
		return nil
	}
	var c githubConfig
	if err := json.Unmarshal(data, &c); err != nil {
		return nil
	}
	return &c
}

func saveGithubConfig(c *githubConfig) error {
	p := githubConfigPath()
	if err := os.MkdirAll(filepath.Dir(p), 0700); err != nil {
		return err
	}
	data, _ := json.MarshalIndent(c, "", "  ")
	return os.WriteFile(p, data, 0600)
}

// ─── helpers ─────────────────────────────────────────────────────────────────

func githubProjectRef() string {
	s := config.SupabaseURL
	s = strings.TrimPrefix(s, "https://")
	s = strings.TrimSuffix(s, ".supabase.co")
	return s
}

func githubWebhookURL() string {
	return fmt.Sprintf("https://%s.supabase.co/functions/v1/github-webhook", githubProjectRef())
}

func commandAvailable(name string) bool {
	_, err := exec.LookPath(name)
	return err == nil
}

func getRepoFullName() string {
	remote := gitOutput("git", "remote", "get-url", "origin")
	// matches github.com:owner/repo or github.com/owner/repo
	for _, rem := range []string{remote} {
		idx := strings.Index(rem, "github.com")
		if idx < 0 {
			continue
		}
		rest := rem[idx+len("github.com"):]
		rest = strings.TrimLeft(rest, ":/")
		rest = strings.TrimSuffix(rest, ".git")
		if parts := strings.SplitN(rest, "/", 2); len(parts) == 2 {
			return parts[0] + "/" + parts[1]
		}
	}
	return ""
}

func findFunctionsDir() string {
	cwd, _ := os.Getwd()
	candidates := []string{
		cwd,
		filepath.Join(cwd, ".."),
		filepath.Join(cwd, "..", "functions"),
		filepath.Join(cwd, "functions"),
	}
	for _, dir := range candidates {
		check := filepath.Join(dir, "supabase", "functions", "github-webhook", "index.ts")
		if _, err := os.Stat(check); err == nil {
			return dir
		}
	}
	return ""
}

func createWebhookViaGh(repo, webhookURL, secret string) bool {
	payload, _ := json.Marshal(map[string]any{
		"name":   "web",
		"active": true,
		"events": []string{"pull_request"},
		"config": map[string]any{
			"url":          webhookURL,
			"content_type": "json",
			"secret":       secret,
			"insecure_ssl": "0",
		},
	})
	cmd := exec.Command("gh", "api", fmt.Sprintf("repos/%s/hooks", repo),
		"--method", "POST", "--input", "-")
	cmd.Stdin = strings.NewReader(string(payload))
	out, err := cmd.CombinedOutput()
	if err != nil {
		s := string(out)
		if strings.Contains(s, "422") || strings.Contains(s, "already exists") {
			return true
		}
		fmt.Fprintf(os.Stderr, "  gh error: %s\n", strings.TrimSpace(s))
		return false
	}
	return true
}

// ─── subcommands ─────────────────────────────────────────────────────────────

func githubSetup() error {
	fmt.Fprint(os.Stderr, "\nPCR GitHub integration setup\n\n")

	cfg := loadGithubConfig()
	var secret string

	if cfg != nil && cfg.WebhookSecret != "" {
		fmt.Fprintln(os.Stderr, "  Webhook secret already generated. Reusing it.")
		secret = cfg.WebhookSecret
	} else {
		b := make([]byte, 32)
		_, _ = rand.Read(b)
		secret = hex.EncodeToString(b)
		cfg = &githubConfig{
			WebhookSecret: secret,
			ConfiguredAt:  time.Now().UTC().Format(time.RFC3339),
		}
		if err := saveGithubConfig(cfg); err != nil {
			return fmt.Errorf("failed to save github config: %w", err)
		}
		fmt.Fprintln(os.Stderr, "  Generated webhook secret.")
	}

	webhookURL := githubWebhookURL()
	projectRef := githubProjectRef()

	// Deploy Edge Function
	if commandAvailable("supabase") {
		functionsDir := findFunctionsDir()
		if functionsDir != "" {
			fmt.Fprintln(os.Stderr, "\n  Deploying Edge Function...")
			cmd := exec.Command("supabase", "functions", "deploy", "github-webhook",
				"--project-ref", projectRef, "--no-verify-jwt")
			cmd.Dir = functionsDir
			cmd.Stdout = os.Stderr
			cmd.Stderr = os.Stderr
			if err := cmd.Run(); err != nil {
				fmt.Fprintln(os.Stderr, "  Deploy failed — the function may already be up to date.")
			} else {
				fmt.Fprintln(os.Stderr, "  Edge Function deployed.")
			}
		} else {
			fmt.Fprintln(os.Stderr, "\n  Could not locate function source — skipping deploy.")
			fmt.Fprintf(os.Stderr, "  To deploy manually, run from the functions/ directory:\n")
			fmt.Fprintf(os.Stderr, "  supabase functions deploy github-webhook --project-ref %s --no-verify-jwt\n", projectRef)
		}

		// Set secret
		fmt.Fprintln(os.Stderr, "\n  Setting webhook secret in Supabase...")
		cmd := exec.Command("supabase", "secrets", "set",
			fmt.Sprintf("GITHUB_WEBHOOK_SECRET=%s", secret),
			"--project-ref", projectRef)
		cmd.Stdout = os.Stderr
		cmd.Stderr = os.Stderr
		if err := cmd.Run(); err != nil {
			fmt.Fprintln(os.Stderr, "  Failed to set secret. Run manually:")
			fmt.Fprintf(os.Stderr, "  supabase secrets set GITHUB_WEBHOOK_SECRET=%s --project-ref %s\n", secret, projectRef)
		} else {
			fmt.Fprintln(os.Stderr, "  Secret set.")
		}
	} else {
		fmt.Fprintln(os.Stderr, "\n  Supabase CLI not found — skipping deploy and secret set.")
		fmt.Fprintf(os.Stderr, "  Run these manually from the functions/ directory:\n")
		fmt.Fprintf(os.Stderr, "  supabase functions deploy github-webhook --project-ref %s --no-verify-jwt\n", projectRef)
		fmt.Fprintf(os.Stderr, "  supabase secrets set GITHUB_WEBHOOK_SECRET=%s --project-ref %s\n", secret, projectRef)
	}

	// Create GitHub webhook
	repoFullName := getRepoFullName()
	fmt.Fprintln(os.Stderr, "\n  Setting up GitHub webhook...")
	if repoFullName == "" {
		fmt.Fprintln(os.Stderr, "  Could not detect GitHub repo from git remote.")
	} else if commandAvailable("gh") {
		fmt.Fprintf(os.Stderr, "  Creating webhook on %s...\n", repoFullName)
		if ok := createWebhookViaGh(repoFullName, webhookURL, secret); ok {
			fmt.Fprintf(os.Stderr, "  Webhook created on github.com/%s\n", repoFullName)
		} else {
			fmt.Fprintln(os.Stderr, "  gh API call failed — opening GitHub in your browser instead.")
			openBrowser(fmt.Sprintf("https://github.com/%s/settings/hooks/new", repoFullName))
		}
	} else {
		fmt.Fprintln(os.Stderr, "  gh CLI not found — opening GitHub in your browser.")
		if repoFullName != "" {
			openBrowser(fmt.Sprintf("https://github.com/%s/settings/hooks/new", repoFullName))
		}
	}

	fmt.Fprintf(os.Stderr, `
  ─────────────────────────────────────────────────────

  Webhook URL:    %s
  Webhook secret: %s

  ─────────────────────────────────────────────────────

  Last step: connect your GitHub account at %s/settings
`, webhookURL, secret, config.AppURL)
	return nil
}

func githubStatus() error {
	cfg := loadGithubConfig()
	webhookURL := githubWebhookURL()

	fmt.Fprint(os.Stderr, "\nPCR GitHub integration status\n\n")
	if cfg != nil {
		t, _ := time.Parse(time.RFC3339, cfg.ConfiguredAt)
		fmt.Fprintf(os.Stderr, "  Webhook secret:  configured (set %s)\n", t.Format("2006-01-02"))
	} else {
		fmt.Fprintln(os.Stderr, "  Webhook secret:  not configured — run `pcr github setup`")
	}
	fmt.Fprintf(os.Stderr, "  Webhook URL:     %s\n", webhookURL)
	fmt.Fprintf(os.Stderr, "  Connect GitHub:  %s/settings\n\n", config.AppURL)
	return nil
}
