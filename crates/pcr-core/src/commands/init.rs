//! `pcr init`. Registers the cwd (or child git repos) as tracked projects.
//! Mirrors `cli/cmd/init.go`.

use std::path::Path;

use crate::agent::OutputMode;
use crate::auth;
use crate::display;
use crate::entry::InitArgs;
use crate::exit::ExitCode;
use crate::projects;
use crate::sources::shared::git;
use crate::supabase;
use crate::util::text::plural;

pub fn run(_mode: OutputMode, args: InitArgs) -> ExitCode {
    let Ok(project_path) = std::env::current_dir() else {
        return ExitCode::GenericError;
    };
    let project_path = project_path.to_string_lossy().into_owned();

    if args.unregister {
        if projects::unregister(&project_path) {
            display::eprintln(&format!("PCR: Unregistered {project_path}"));
        } else {
            display::eprintln(&format!("PCR: {project_path} was not registered."));
        }
        return ExitCode::Success;
    }

    if is_git_repo(&project_path) {
        register_one(&project_path);
        display::eprintln("\nPCR: Run `pcr start` to begin capturing prompts.");
        return ExitCode::Success;
    }

    // Scan immediate subdirs for git repos.
    let entries = match std::fs::read_dir(&project_path) {
        Ok(e) => e,
        Err(e) => {
            display::print_error("init", &e.to_string());
            return ExitCode::GenericError;
        }
    };
    let mut found = Vec::<String>::new();
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let sub = entry.path().to_string_lossy().into_owned();
        if is_git_repo(&sub) {
            found.push(sub);
        }
    }

    if found.is_empty() {
        display::eprintln("PCR: No git repositories found in the current directory.");
        display::print_hint("cd into a git repo (`git status` should work) and try again");
        return ExitCode::Success;
    }

    display::eprintln(&format!(
        "PCR: Found {} git repo{} — registering all.\n",
        found.len(),
        plural(found.len())
    ));
    for sub in &found {
        register_one(sub);
        display::eprintln("");
    }
    display::eprintln("PCR: Run `pcr start` to begin capturing prompts.");
    ExitCode::Success
}

fn register_one(project_path: &str) {
    let git_remote = git::git_output_in(project_path, &["remote", "get-url", "origin"]);
    let project = projects::register(project_path);
    display::eprintln(&format!("  ✓ {}", project.name));
    display::eprintln(&format!("    Path:        {}", project_path));
    display::eprintln(&format!("    Cursor slug: {}", project.cursor_slug));

    let a = auth::load();
    if let Some(a) = a.as_ref() {
        if !git_remote.is_empty() {
            match supabase::register_project(
                "",
                &project.name,
                &git_remote,
                project_path,
                &a.user_id,
            ) {
                Ok(project_id) if !project_id.is_empty() => {
                    projects::update_project_id(project_path, &project_id);
                    display::eprintln(&format!("    Remote ID:   {project_id}"));
                }
                Ok(_) => {}
                Err(e) => {
                    display::eprintln(&format!("    Remote:      failed ({e})"));
                }
            }
        } else {
            display::eprintln("    Remote:      skipped (no git remote)");
        }
    } else {
        display::eprintln("    Remote:      skipped (not logged in — run `pcr login`)");
    }
}

fn is_git_repo(dir: &str) -> bool {
    Path::new(dir).join(".git").exists()
}
