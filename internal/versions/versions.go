package versions

import (
	"os/exec"
	"strings"
)

const CaptureSchemaVersion = 3

// CursorVersion attempts to detect the installed Cursor version.
func CursorVersion() string {
	// Try reading Cursor's package.json via cursor CLI
	cmd := exec.Command("cursor", "--version")
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// PCRVersion returns the current PCR CLI version (injected at build time).
// This is a placeholder; the actual version is in main.Version.
func PCRVersion(v string) string {
	return v
}
