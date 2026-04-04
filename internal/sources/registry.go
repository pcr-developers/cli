package sources

import (
	"github.com/pcr-developers/cli/internal/sources/claudecode"
	"github.com/pcr-developers/cli/internal/sources/cursor"
)

// All returns all registered capture sources with the shared DiffTracker wired in.
func All(dt *DiffTracker) []CaptureSource {
	return []CaptureSource{
		&claudecode.Source{},
		&cursor.Source{DiffTracker: dt}, // DiffTracker satisfies cursor.Poller
	}
}
