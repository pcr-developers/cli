package sources

// CaptureSource defines the interface for an IDE/AI agent capture source.
type CaptureSource interface {
	// Name returns the display name (e.g. "Claude Code").
	Name() string
	// Start begins watching for new sessions. Blocks until the watcher stops.
	Start(userID string)
}
