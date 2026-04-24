#![deny(clippy::all)]

use napi_derive::napi;

/// Run the PCR CLI with the given argv. Returns the process exit code.
///
/// This is what the npm `bin/pcr` shim calls. Because the Rust code executes
/// inside `node.exe` via `LoadLibrary`, Windows AppLocker never evaluates it
/// as a separate process — sidestepping the default "deny exec from %AppData%"
/// rule that blocks a shipped .exe.
#[napi]
pub fn run(argv: Vec<String>) -> i32 {
    pcr_core::entry::run(argv)
}
