package shared

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"

	"github.com/pcr-developers/cli/internal/config"
)

// FileState tracks how many lines have been processed per file.
type FileState struct {
	mu       sync.Mutex
	data     map[string]int // file path → lines processed
	filePath string
}

func NewFileState(name string) *FileState {
	home, _ := os.UserHomeDir()
	return &FileState{
		data:     map[string]int{},
		filePath: filepath.Join(home, config.PCRDir, name+"-state.json"),
	}
}

func (s *FileState) Load() {
	s.mu.Lock()
	defer s.mu.Unlock()
	data, err := os.ReadFile(s.filePath)
	if err != nil {
		return
	}
	_ = json.Unmarshal(data, &s.data)
}

func (s *FileState) Get(path string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.data[path]
}

func (s *FileState) Set(path string, lines int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.data[path] = lines
	s.persist()
}

func (s *FileState) persist() {
	data, err := json.MarshalIndent(s.data, "", "  ")
	if err != nil {
		return
	}
	_ = os.MkdirAll(filepath.Dir(s.filePath), 0755)
	_ = os.WriteFile(s.filePath, data, 0644)
}
