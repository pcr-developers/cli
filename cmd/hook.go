package cmd

import (
	"github.com/spf13/cobra"

	"github.com/pcr-developers/cli/internal/sources/claudecode"
)

// hookCmd is called by Claude Code's Stop hook after every response.
// It opens /dev/tty directly so it works even when the tool holds stdin.
// Always exits 0 — never re-engages the model.
var hookCmd = &cobra.Command{
	Use:    "hook",
	Short:  "Internal: called by Claude Code's Stop hook after each response",
	Hidden: true,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Only act if pcr start is actually running.
		if _, alive := readExistingPID(pidFilePath()); !alive {
			return nil
		}
		ctx := resolveProjectContext()
		return claudecode.RunHook(ctx.ids, ctx.names, ctx.name)
	},
}
