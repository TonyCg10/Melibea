//! `melibea-ruler`: a one-shot screen ruler for niri.
//!
//! See `docs/superpowers/specs/2026-10-08-melibea-ruler-design.md`.

mod capture;
mod cli;
mod draw;
mod edges;
mod image;
mod lock;
mod overlay;
mod text;
mod units;

use std::process::ExitCode;

use capture::Capture;
use cli::{Command, Mode};
use wayland_client::Connection;

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("melibea-ruler: {message}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Measure {
            mode,
            at,
            output,
            tolerance,
        } => headless(mode, at, &output, tolerance),
        Command::Interactive { tolerance } => interactive(tolerance),
    }
}

/// Freezes the screen, lets one thing be measured, copies it, and exits.
fn interactive(tolerance: u8) -> ExitCode {
    let _lock = match lock::acquire() {
        Ok(Some(lock)) => lock,
        // Another ruler is open: a second one would capture its guides.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => return fail(&message),
    };
    let conn = match Connection::connect_to_env() {
        Ok(conn) => conn,
        Err(e) => return fail(&format!("cannot connect to the compositor: {e}")),
    };
    // Everything that can fail before anything is shown, fails here.
    let captures = match capture::capture_all(&conn) {
        Ok(captures) => captures,
        Err(message) => return fail(&message),
    };
    let text = match text::Text::load() {
        Ok(text) => text,
        Err(message) => return fail(&message),
    };
    match overlay::run(&conn, captures, tolerance, text) {
        overlay::Outcome::Copy(result) => copy(&result),
        overlay::Outcome::Cancel => ExitCode::SUCCESS,
        overlay::Outcome::Fail(message) => fail(&message),
    }
}

fn copy(result: &str) -> ExitCode {
    use std::io::Write as _;
    let spawned = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(result.as_bytes())?;
            }
            child.wait()
        });
    match spawned {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => fail(&format!("wl-copy exited with {status}; {result} was not copied")),
        Err(e) => fail(&format!("cannot run wl-copy ({e}); {result} was not copied")),
    }
}

/// A failure launched from a bind is otherwise invisible, so it is also notified.
fn fail(message: &str) -> ExitCode {
    eprintln!("melibea-ruler: {message}");
    let _ = std::process::Command::new("notify-send")
        .args(["Regla", message])
        .status();
    ExitCode::FAILURE
}

/// Measures at one point, prints the result and exits, showing nothing.
fn headless(mode: Mode, at: (f64, f64), output: &str, tolerance: u8) -> ExitCode {
    let result = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the compositor: {e}"))
        .and_then(|conn| capture::capture_all(&conn))
        .and_then(|captures| {
            let capture = captures.iter().find(|c| c.name == output).ok_or_else(|| {
                let names: Vec<&str> = captures.iter().map(|c| c.name.as_str()).collect();
                format!("no output named {output:?}; there are {names:?}")
            })?;
            let scale = capture.scale();
            let point = (
                units::to_physical(at.0, scale, capture.image.width()),
                units::to_physical(at.1, scale, capture.image.height()),
            );
            let (width, height) = measure(capture, mode, point, tolerance);
            Ok(units::clipboard(width, height))
        });
    match result {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("melibea-ruler: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The measurement at a physical point, in logical pixels.
fn measure(capture: &Capture, mode: Mode, at: (u32, u32), tolerance: u8) -> (u32, u32) {
    let bounds = match mode {
        Mode::Box => edges::region(&capture.image, at.0, at.1, tolerance).bounds(),
        Mode::Spacing => edges::spacing(&capture.image, at.0, at.1, tolerance),
    };
    let scale = capture.scale();
    (
        units::to_logical(bounds.width(), scale),
        units::to_logical(bounds.height(), scale),
    )
}
