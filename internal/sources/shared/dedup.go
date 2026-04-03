package shared

import "sync"

// Deduplicator tracks content hashes seen in this watcher run to avoid
// redundant Supabase upserts within a single session.
type Deduplicator struct {
	mu   sync.Mutex
	seen map[string]map[string]bool // sessionID → set of content hashes
}

func NewDeduplicator() *Deduplicator {
	return &Deduplicator{seen: map[string]map[string]bool{}}
}

// IsDuplicate returns true if this hash has already been seen for the session.
func (d *Deduplicator) IsDuplicate(sessionID, hash string) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.seen[sessionID] == nil {
		return false
	}
	return d.seen[sessionID][hash]
}

// Mark records a hash as seen for the session.
func (d *Deduplicator) Mark(sessionID, hash string) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.seen[sessionID] == nil {
		d.seen[sessionID] = map[string]bool{}
	}
	d.seen[sessionID][hash] = true
}
