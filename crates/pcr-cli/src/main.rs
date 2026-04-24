use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let code = pcr_core::entry::run(argv);
    ExitCode::from(code.clamp(0, 255) as u8)
}
