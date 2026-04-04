package cmd

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/projects"
	"github.com/pcr-developers/cli/internal/sources/cursor"
	"github.com/pcr-developers/cli/internal/store"
)

func formatCapturedAt(s string) string {
	t, err := time.Parse(time.RFC3339, s)
	if err != nil {
		t, err = time.Parse("2006-01-02T15:04:05.999Z", s)
	}
	if err != nil {
		return ""
	}
	return t.Local().Format("Jan 2 15:04")
}

// syncLatestPrompts forces a crawl of the most recent Cursor transcripts so
// pcr bundle always shows the latest prompts — including exchanges the
// background watcher hasn't picked up yet.
// Also records any file changes since the last DiffTracker background poll
// so attribution is current for the latest exchange.
func syncLatestPrompts() {
	fmt.Fprint(os.Stderr, "Fetching latest prompts...\r")

	// Record current git state for all registered projects so diff_events
	// is up to date before the force sync queries it for attribution.
	// This handles the case where the DiffTracker hasn't polled yet.
	for _, p := range projects.Load() {
		if p.Path == "" || p.ProjectID == "" {
			continue
		}
		// Use a one-shot inline poll via git status --porcelain.
		// The DiffTracker will deduplicate against its own state; we just
		// ensure the events are in the DB before the per-bubble queries run.
		out, _ := exec.Command("git", "-C", p.Path, "status", "--porcelain").Output()
		if len(out) > 0 {
			var files []string
			for _, line := range filterNonEmpty(strings.Split(string(out), "\n")) {
				if len(line) >= 4 {
					files = append(files, filepath.Join(p.Path, strings.TrimSpace(line[3:])))
				}
			}
			if len(files) > 0 {
				_ = store.RecordDiffEvent(p.ProjectID, p.Name, files, time.Now())
			}
		}
	}

	cursor.ForceSync("", nil, 5)
	fmt.Fprint(os.Stderr, "                          \r")
}

// retagDraftsNow re-attributes all local drafts using the DiffTracker's event log.
// For each draft we query DiffEvents in the window [prev_captured_at, captured_at + 2min]
// to find which repos had file changes during that prompt's response time.
// This gives precise per-prompt attribution without any git commands running here.
func retagDraftsNow() {
	ctx := resolveProjectContext()
	drafts, err := store.GetAllDraftsSortedByTime()
	if err != nil || len(drafts) == 0 {
		return
	}

	for i, d := range drafts {
		capturedAt, err := parseDraftTime(d.CapturedAt)
		if err != nil {
			continue
		}

		// Window: [this prompt's captured_at, next prompt's captured_at].
		// Same logic as the real-time watcher — files changed between T_N and
		// T_{N+1} were the AI's response to prompt N.
		// For the last prompt: add a 5-minute buffer (AI may still be responding).
		var windowEnd time.Time
		if i+1 < len(drafts) {
			windowEnd, _ = parseDraftTime(drafts[i+1].CapturedAt)
		}
		if windowEnd.IsZero() {
			windowEnd = capturedAt.Add(5 * time.Minute)
		}

		// Window start: this prompt's captured_at (events before it belong to the previous prompt).
		windowStart := capturedAt

		events, err := store.GetDiffEventsInWindow(windowStart, windowEnd)
		if err != nil || len(events) == 0 {
			continue
		}

		// Only consider events for projects registered in the current workspace.
		projectHits := map[string]int{}
		for _, e := range events {
			for _, id := range ctx.ids {
				if e.ProjectID == id {
					projectHits[e.ProjectID] += len(e.Files)
				}
			}
		}
		if len(projectHits) == 0 {
			continue
		}

		// Primary = project with most file changes.
		var primaryID, primaryName string
		var allIDs []string
		for id, count := range projectHits {
			allIDs = append(allIDs, id)
			if primaryID == "" || count > projectHits[primaryID] {
				primaryID = id
				primaryName = projNameForID(id)
			}
		}

		_ = store.UpsertDraftProject(d.ContentHash, primaryID, primaryName, allIDs)
	}
}

// parseDraftTime parses a draft's captured_at field which may include
// milliseconds ("2026-04-04T18:22:05.044Z") not handled by time.RFC3339.
func parseDraftTime(s string) (time.Time, error) {
	t, err := time.Parse(time.RFC3339, s)
	if err == nil {
		return t, nil
	}
	return time.Parse("2006-01-02T15:04:05.999Z", s)
}

// projNameForID looks up a project name from the local registry by project ID.
func projNameForID(id string) string {
	for _, p := range projects.Load() {
		if p.ProjectID == id {
			return p.Name
		}
	}
	return ""
}

// ─── Repo attribution helpers ─────────────────────────────────────────────────

func loadProjByID() map[string]string {
	m := map[string]string{}
	for _, p := range projects.Load() {
		if p.ProjectID != "" {
			m[p.ProjectID] = p.Name
		}
	}
	return m
}

func repoBadge(d store.DraftRecord, projByID map[string]string) string {
	touchedIDs := d.TouchedProjectIDs()
	if len(touchedIDs) > 1 {
		var names []string
		for _, id := range touchedIDs {
			if name, ok := projByID[id]; ok {
				names = append(names, name)
			}
		}
		if len(names) > 0 {
			return "[" + strings.Join(names, ",") + "]"
		}
	}
	if d.ProjectName != "" {
		return "[" + d.ProjectName + "]"
	}
	return ""
}

func filterByRepo(drafts []store.DraftRecord, repoName string, projByID map[string]string) []store.DraftRecord {
	if repoName == "" {
		return drafts
	}
	var targetID string
	for id, name := range projByID {
		if strings.EqualFold(name, repoName) {
			targetID = id
			break
		}
	}
	var result []store.DraftRecord
	for _, d := range drafts {
		if strings.EqualFold(d.ProjectName, repoName) {
			result = append(result, d)
			continue
		}
		if targetID != "" {
			for _, id := range d.TouchedProjectIDs() {
				if id == targetID {
					result = append(result, d)
					break
				}
			}
		}
	}
	return result
}

var bundleCmd = &cobra.Command{
	Use:   "bundle [name]",
	Short: "Create and manage prompt bundles",
	Long: `Create a prompt bundle from captured drafts, or manage existing bundles.

With a name and --select: creates the bundle immediately (auto-sealed, ready to push).
With no args: shows all drafts and unpushed bundles.

Examples:
  pcr bundle                                  # show drafts + bundles
  pcr bundle "auth fix" --select 1-5          # create bundle from drafts 1-5
  pcr bundle "auth fix" --select all          # bundle all drafts
  pcr bundle "auth fix" --add --select 6,7    # add more prompts to existing bundle
  pcr bundle "auth fix" --remove --select 2   # remove prompt 2 from bundle
  pcr bundle "auth fix" --delete              # delete bundle, return prompts to drafts
  pcr bundle --list                           # list all unpushed bundles`,
	RunE: func(cmd *cobra.Command, args []string) error {
		listFlag, _ := cmd.Flags().GetBool("list")
		deleteFlag, _ := cmd.Flags().GetBool("delete")
		addFlag, _ := cmd.Flags().GetBool("add")
		removeFlag, _ := cmd.Flags().GetBool("remove")
		selectArg, _ := cmd.Flags().GetString("select")
		repoFilter, _ := cmd.Flags().GetString("repo")

		name := strings.TrimSpace(strings.Join(args, " "))

		// pcr bundle --list
		if listFlag {
			return runBundleList()
		}

		// pcr bundle "name" --delete
		if deleteFlag {
			if name == "" {
				return fmt.Errorf("--delete requires a bundle name: pcr bundle \"name\" --delete")
			}
			return runBundleDelete(name)
		}

		// pcr bundle "name" --remove --select 1,2
		if removeFlag {
			if name == "" {
				return fmt.Errorf("--remove requires a bundle name: pcr bundle \"name\" --remove --select 1,2")
			}
			if selectArg == "" {
				return fmt.Errorf("--remove requires --select: pcr bundle %q --remove --select 1,2", name)
			}
			return runBundleRemove(name, selectArg)
		}

		// pcr bundle "name" --add --select 6,7
		if addFlag {
			if name == "" {
				return fmt.Errorf("--add requires a bundle name: pcr bundle \"name\" --add --select 1-5")
			}
			if selectArg == "" {
				return fmt.Errorf("--add requires --select: pcr bundle %q --add --select 1-5", name)
			}
			return runBundleAdd(name, selectArg)
		}

		// pcr bundle "name" --select 1-5  →  create new bundle (auto-sealed)
		if name != "" && selectArg != "" {
			return runBundleCreate(name, selectArg, repoFilter)
		}

		// pcr bundle "name" with no --select:
		// In a real external terminal (Terminal.app, iTerm2): show list, prompt inline.
		// In Cursor terminals (integrated tabs, agent shell): show list + --select hint.
		if name != "" {
			if isInteractiveTerminal() {
				return runBundleInteractive(name, repoFilter)
			}
			return runBundleShowHint(name, repoFilter)
		}

		// pcr bundle  →  show drafts + bundles overview
		return runBundleOverview(repoFilter)
	},
}

// forced poll test - final cli
// genericBundleNames are rejected as bundle names — they're placeholders people
// type by accident and create useless bundles with no meaningful label.
var genericBundleNames = map[string]bool{
	"name": true, "test": true, "bundle": true, "prompt bundle": true,
	"my bundle": true, "untitled": true, "draft": true, "temp": true,
}

// runBundleCreate creates a new sealed bundle from selected drafts.
// repoFilter, if set, narrows the draft pool to only prompts that touched that repo.
// Draft numbers shown in the overview always correspond to the (possibly filtered) pool.
func runBundleCreate(name, selectArg, repoFilter string) error {
	if genericBundleNames[strings.ToLower(strings.TrimSpace(name))] {
		fmt.Fprintf(os.Stderr, "PCR: %q is not a useful bundle name — describe what you actually changed.\n", name)
		fmt.Fprintln(os.Stderr, `     Example: pcr bundle "fix interactive terminal in Cursor" --select all`)
		return nil
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
	all := filterByRepo(append(drafts, staged...), repoFilter, loadProjByID())

	if len(all) == 0 {
		if repoFilter != "" {
			fmt.Fprintf(os.Stderr, "PCR: No draft prompts attributed to repo %q.\n", repoFilter)
		} else {
			fmt.Fprintln(os.Stderr, "PCR: No draft prompts available. Run `pcr start` to capture prompts.")
		}
		return nil
	}

	var selected []store.DraftRecord
	if strings.ToLower(selectArg) == "all" {
		selected = all
	} else {
		selected = parseSelection(selectArg, all)
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing bundled.")
		return nil
	}

	projectID := ""
	projectName := ctx.name
	if len(ctx.ids) > 0 {
		projectID = ctx.ids[0]
	}
	branch := gitOutput("git", "rev-parse", "--abbrev-ref", "HEAD")
	// If the current dir isn't a git repo (e.g. pcr-developers/ org folder),
	// try to get the branch from the primary project's path.
	if branch == "" && projectID != "" {
		for _, p := range projects.Load() {
			if p.ProjectID == projectID && p.Path != "" {
				branch = gitOutputIn(p.Path, "git", "rev-parse", "--abbrev-ref", "HEAD")
				break
			}
		}
	}
	sha := "bundle-" + generateID()

	// "closed" = auto-sealed, ready to push
	_, err = store.CreateCommit(name, sha, draftIDs(selected), projectID, projectName, branch, "closed")
	if err != nil {
		return err
	}

	fmt.Fprintf(os.Stderr, "PCR: Created prompt bundle %q (%d prompt%s) — push with `pcr push`\n",
		name, len(selected), plural(len(selected)))
	return nil
}

// runBundleAdd adds more drafts to an existing bundle.
func runBundleAdd(name, selectArg string) error {
	bundle, err := store.GetBundleByName(name)
	if err != nil {
		return err
	}
	if bundle == nil {
		return fmt.Errorf("no bundle named %q — create it first with: pcr bundle %q --select 1-5", name, name)
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
		fmt.Fprintln(os.Stderr, "PCR: No draft prompts available to add.")
		return nil
	}

	var selected []store.DraftRecord
	if strings.ToLower(selectArg) == "all" {
		selected = all
	} else {
		selected = parseSelection(selectArg, all)
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing added.")
		return nil
	}

	if err := store.AddDraftsToBundle(bundle.ID, draftIDs(selected)); err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Added %d prompt%s to %q — push with `pcr push`\n",
		len(selected), plural(len(selected)), name)
	return nil
}

// runBundleRemove removes specific prompts from a bundle (returns them to drafts).
func runBundleRemove(name, selectArg string) error {
	bundle, err := store.GetBundleByName(name)
	if err != nil {
		return err
	}
	if bundle == nil {
		return fmt.Errorf("no bundle named %q", name)
	}

	full, err := store.GetCommitWithItems(bundle.ID)
	if err != nil {
		return err
	}
	if len(full.Items) == 0 {
		fmt.Fprintf(os.Stderr, "PCR: Bundle %q is empty.\n", name)
		return nil
	}

	var selected []store.DraftRecord
	if strings.ToLower(selectArg) == "all" {
		selected = full.Items
	} else {
		selected = parseSelection(selectArg, full.Items)
	}
	if len(selected) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No valid selection — nothing removed.")
		return nil
	}

	if err := store.RemoveDraftsFromBundle(bundle.ID, draftIDs(selected)); err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Removed %d prompt%s from %q — they're back in drafts.\n",
		len(selected), plural(len(selected)), name)
	return nil
}

// runBundleDelete deletes a bundle and returns its prompts to drafts.
func runBundleDelete(name string) error {
	bundle, err := store.GetBundleByName(name)
	if err != nil {
		return err
	}
	if bundle == nil {
		return fmt.Errorf("no bundle named %q", name)
	}
	if err := store.DeleteBundle(bundle.ID); err != nil {
		return err
	}
	fmt.Fprintf(os.Stderr, "PCR: Deleted bundle %q — prompts returned to drafts.\n", name)
	return nil
}

// runBundleList lists all unpushed bundles with their prompt counts.
func runBundleList() error {
	unpushed, err := store.GetUnpushedCommits()
	if err != nil {
		return err
	}
	if len(unpushed) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No unpushed bundles — everything is pushed.")
		return nil
	}

	const bold = "\x1b[1m"
	const dim = "\x1b[2m"
	const yel = "\x1b[33m"
	const grn = "\x1b[32m"
	const rst = "\x1b[0m"

	fmt.Fprintf(os.Stderr, "%sUnpushed prompt bundles%s  (%d)\n\n", bold, rst, len(unpushed))
	for _, b := range unpushed {
		full, _ := store.GetCommitWithItems(b.ID)
		count := 0
		if full != nil {
			count = len(full.Items)
		}
		marker := grn + "✓" + rst
		status := "sealed"
		if b.BundleStatus == "open" {
			marker = yel + "~" + rst
			status = "open"
		}
		fmt.Fprintf(os.Stderr, "  %s  %s%q%s  %s(%d prompt%s, %s)%s\n",
			marker, bold, b.Message, rst, dim, count, plural(count), status, rst)
	}
	fmt.Fprintln(os.Stderr)
	fmt.Fprintf(os.Stderr, "  %spcr push%s   push all sealed bundles\n", yel, rst)
	return nil
}

// runBundleInteractive shows the draft list and reads selection from stdin.
// Only called when isInteractiveTerminal() is true (real terminal, not Cursor).
func runBundleInteractive(name, repoFilter string) error {
	ctx := resolveProjectContext()
	drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
	if err != nil {
		return err
	}
	staged, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	projByID := loadProjByID()
	all := filterByRepo(append(drafts, staged...), repoFilter, projByID)

	if len(all) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No draft prompts available.")
		return nil
	}

	const dim = "\x1b[2m"
	const bold = "\x1b[1m"
	const cyn = "\x1b[36m"
	const rst = "\x1b[0m"

	title := "Draft prompts"
	if repoFilter != "" {
		title += "  (repo: " + repoFilter + ")"
	}
	fmt.Fprintf(os.Stderr, "%s%s%s  (%d)\n\n", bold, title, rst, len(all))
	for idx, d := range all {
		date := formatCapturedAt(d.CapturedAt)
		badge := repoBadge(d, projByID)
		badgeFmt := ""
		if badge != "" {
			badgeFmt = " " + cyn + badge + rst
		}
		fmt.Fprintf(os.Stderr, "  [%d] %s%s%s%s %q\n", idx+1, dim, date, rst, badgeFmt, promptPreview(d.PromptText, 55))
	}
	fmt.Fprintln(os.Stderr)

	fmt.Fprint(os.Stderr, "Select prompts [e.g. 1-5, 1,3,7, or all — enter to cancel]: ")
	reader := bufio.NewReader(os.Stdin)
	line, err := reader.ReadString('\n')
	if err != nil {
		fmt.Fprintln(os.Stderr, "\nPCR: Cancelled.")
		return nil
	}
	resp := strings.TrimSpace(line)
	if resp == "" {
		fmt.Fprintln(os.Stderr, "PCR: Cancelled.")
		return nil
	}

	return runBundleCreate(name, resp, repoFilter)
}

// runBundleShowHint shows the draft list with a hint to use --select.
func runBundleShowHint(name, repoFilter string) error {
	syncLatestPrompts()
	retagDraftsNow()
	ctx := resolveProjectContext()
	drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
	if err != nil {
		return err
	}
	staged, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	projByID := loadProjByID()
	all := filterByRepo(append(drafts, staged...), repoFilter, projByID)

	if len(all) == 0 {
		fmt.Fprintln(os.Stderr, "PCR: No draft prompts available.")
		return nil
	}

	const dim = "\x1b[2m"
	const bold = "\x1b[1m"
	const yel = "\x1b[33m"
	const cyn = "\x1b[36m"
	const rst = "\x1b[0m"

	title := "Draft prompts"
	if repoFilter != "" {
		title += "  (repo: " + repoFilter + ")"
	}
	fmt.Fprintf(os.Stderr, "%s%s%s  (%d)\n\n", bold, title, rst, len(all))
	for idx, d := range all {
		date := formatCapturedAt(d.CapturedAt)
		badge := repoBadge(d, projByID)
		badgeFmt := ""
		if badge != "" {
			badgeFmt = " " + cyn + badge + rst
		}
		fmt.Fprintf(os.Stderr, "  [%d] %s%s%s%s %q\n", idx+1, dim, date, rst, badgeFmt, promptPreview(d.PromptText, 55))
	}
	fmt.Fprintln(os.Stderr)
	repoSuffix := ""
	if repoFilter != "" {
		repoSuffix = " --repo " + repoFilter
	}
	fmt.Fprintf(os.Stderr, "  %spcr bundle %q --select 1-5%s%s\n", yel, name, repoSuffix, rst)
	fmt.Fprintf(os.Stderr, "  %spcr bundle %q --select all%s%s\n", yel, name, repoSuffix, rst)
	return nil
}

// runBundleOverview shows all drafts and unpushed bundles.
func runBundleOverview(repoFilter string) error {
	syncLatestPrompts()
	retagDraftsNow()
	ctx := resolveProjectContext()
	drafts, err := store.GetDraftsByStatus(store.StatusDraft, ctx.ids, ctx.names)
	if err != nil {
		return err
	}
	staged, err := store.GetStagedDrafts()
	if err != nil {
		return err
	}
	projByID := loadProjByID()
	all := filterByRepo(append(drafts, staged...), repoFilter, projByID)
	unpushed, _ := store.GetUnpushedCommits()

	const bold = "\x1b[1m"
	const dim = "\x1b[2m"
	const yel = "\x1b[33m"
	const cyn = "\x1b[36m"
	const grn = "\x1b[32m"
	const rst = "\x1b[0m"

	if len(all) > 0 {
		title := "Draft prompts"
		if repoFilter != "" {
			title += "  (repo: " + repoFilter + ")"
		}
		fmt.Fprintf(os.Stderr, "%s%s%s  (%d)\n\n", bold, title, rst, len(all))
		for idx, d := range all {
			date := formatCapturedAt(d.CapturedAt)
			badge := repoBadge(d, projByID)
			badgeFmt := ""
			if badge != "" {
				badgeFmt = " " + cyn + badge + rst
			}
			fmt.Fprintf(os.Stderr, "  [%d] %s%s%s%s %q\n", idx+1, dim, date, rst, badgeFmt, promptPreview(d.PromptText, 55))
		}
		fmt.Fprintln(os.Stderr)
	} else if repoFilter != "" {
		fmt.Fprintf(os.Stderr, "%sDrafts%s  0 for repo %q\n\n", bold, rst, repoFilter)
	} else {
		fmt.Fprintf(os.Stderr, "%sDrafts%s  0 — run `pcr start` to capture prompts\n\n", bold, rst)
	}

	if len(unpushed) > 0 {
		fmt.Fprintf(os.Stderr, "%sUnpushed prompt bundles%s  (%d)\n\n", bold, rst, len(unpushed))
		for _, b := range unpushed {
			full, _ := store.GetCommitWithItems(b.ID)
			count := 0
			if full != nil {
				count = len(full.Items)
			}
			marker := grn + "✓" + rst
			if b.BundleStatus == "open" {
				marker = yel + "~" + rst
			}
			fmt.Fprintf(os.Stderr, "  %s  %s%q%s  %s(%d prompt%s)%s\n",
				marker, bold, b.Message, rst, dim, count, plural(count), rst)
		}
		fmt.Fprintln(os.Stderr)
	}

	fmt.Fprintf(os.Stderr, "%sUsage:%s\n", bold, rst)
	fmt.Fprintf(os.Stderr, "  %spcr bundle \"name\" --select 1-5%s            create bundle from drafts 1-5\n", yel, rst)
	fmt.Fprintf(os.Stderr, "  %spcr bundle \"name\" --select all%s            bundle all drafts\n", yel, rst)
	fmt.Fprintf(os.Stderr, "  %spcr bundle \"name\" --select all --repo cli%s  bundle only cli drafts\n", yel, rst)
	fmt.Fprintf(os.Stderr, "  %spcr push%s                                   push all sealed bundles\n", yel, rst)
	fmt.Fprintf(os.Stderr, "  %spcr show <number>%s                          see full text of a draft\n", dim, rst)
	return nil
}

func init() {
	bundleCmd.Flags().String("select", "", "Select drafts by number (e.g. 1-5, 1,3,7, or all)")
	bundleCmd.Flags().Bool("add", false, "Add more prompts to an existing bundle")
	bundleCmd.Flags().Bool("remove", false, "Remove prompts from a bundle")
	bundleCmd.Flags().Bool("delete", false, "Delete a bundle and return its prompts to drafts")
	bundleCmd.Flags().Bool("list", false, "List all unpushed bundles")
	bundleCmd.Flags().String("repo", "", "Filter drafts to only those touching a specific repo (e.g. cli, pcr-dev)")
}
