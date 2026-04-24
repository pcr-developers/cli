//! ID / hash helpers matching the Go `helpers.go` and `supabase` routines.

/// Generate an 8-byte random hex ID (e.g. for `sha := "bundle-" + generateID()`).
/// Matches `cmd/helpers.go::generateID`.
pub fn generate_hex_id() -> String {
    let u = uuid::Uuid::new_v4();
    // First 8 bytes of a v4 UUID are random; that matches Go's `crypto/rand.Read(b[:8])`.
    hex::encode(&u.as_bytes()[..8])
}

/// Generate a new UUIDv4 in the canonical 8-4-4-4-12 format. Matches
/// `store/commits.go::newUUID`.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}
