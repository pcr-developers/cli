//! `pcr logout`. Mirrors `cli/cmd/logout.go`.

use crate::agent::OutputMode;
use crate::auth;
use crate::display;
use crate::exit::ExitCode;

pub fn run(_mode: OutputMode) -> ExitCode {
    auth::clear();
    display::eprintln("PCR: Logged out.");
    ExitCode::Success
}
