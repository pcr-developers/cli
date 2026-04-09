package sources

import (
	"github.com/pcr-developers/cli/internal/sources/claudecode"
	"github.com/pcr-developers/cli/internal/sources/cursor"
)

// All returns all registered capture sources.
func All() []CaptureSource {
	return []CaptureSource{
		&claudecode.Source{},
		&cursor.Source{},
	}
}
