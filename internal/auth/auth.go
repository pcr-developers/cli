package auth

import (
	"encoding/json"
	"os"
	"path/filepath"

	"github.com/pcr-developers/cli/internal/config"
)

type Auth struct {
	Token  string `json:"token"`
	UserID string `json:"userId"`
}

func authFilePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, config.PCRDir, "auth.json")
}

func Load() *Auth {
	data, err := os.ReadFile(authFilePath())
	if err != nil {
		return nil
	}
	var a Auth
	if err := json.Unmarshal(data, &a); err != nil {
		return nil
	}
	return &a
}

func Save(a *Auth) error {
	path := authFilePath()
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(a, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0600)
}

func Clear() {
	_ = os.Remove(authFilePath())
}
