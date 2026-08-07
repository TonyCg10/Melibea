use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("status") => {
            println!("melibea 0.1.0: niri integration is not connected yet");
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") | None => {
            println!("Usage: melibea <status|--help>");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown command: {command}");
            ExitCode::from(2)
        }
    }
}
