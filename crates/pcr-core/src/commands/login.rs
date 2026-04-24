//! `pcr login` — OAuth-style token paste flow. Mirrors `cli/cmd/login.go`.

use std::io::{BufRead, Write};

use crate::agent::OutputMode;
use crate::auth::{self, Auth};
use crate::config;
use crate::display;
use crate::exit::ExitCode;
use crate::supabase;

pub fn run(_mode: OutputMode) -> ExitCode {
    let settings_url = format!("{}/settings", config::APP_URL);
    display::eprintln(&format!(
        "\nPCR: Opening {settings_url} to get your CLI token..."
    ));
    let _ = webbrowser::open(&settings_url);

    display::eprint("Paste your CLI token: ");
    let token = match read_single_line() {
        Some(t) => t.trim().to_string(),
        None => {
            display::print_error("login", "failed to read token");
            return ExitCode::GenericError;
        }
    };
    if token.is_empty() {
        display::print_error("login", "no token provided");
        return ExitCode::Usage;
    }

    display::eprintln("PCR: Validating token...");
    let user_id = match supabase::validate_cli_token(&token) {
        Ok(u) if !u.is_empty() => u,
        _ => {
            display::eprintln(&format!(
                "PCR: Invalid token — please check your token at {settings_url}"
            ));
            return ExitCode::AuthRequired;
        }
    };

    if let Err(e) = auth::save(&Auth {
        token,
        user_id: user_id.clone(),
    }) {
        display::print_error("login", &format!("failed to save credentials: {e}"));
        return ExitCode::GenericError;
    }

    display::eprintln(&format!("PCR: Logged in successfully (user: {user_id})\n"));
    ExitCode::Success
}

/// Read a single line. Prefer `/dev/tty` on Unix when stdin is redirected
/// so `pcr login < token.txt` style usage works; fall back to stdin
/// otherwise. Matches the Go helper's behaviour.
fn read_single_line() -> Option<String> {
    #[cfg(unix)]
    {
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(file);
            if reader.read_line(&mut buf).is_ok() {
                return Some(buf);
            }
        }
    }
    let stdin = std::io::stdin();
    let mut buf = String::new();
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    stdin.lock().read_line(&mut buf).ok()?;
    Some(buf)
}
