//! `melibea-ruler`: a one-shot screen ruler for niri.
//!
//! See `docs/superpowers/specs/2026-10-08-melibea-ruler-design.md`.

#![allow(dead_code)]
// Task 7 removes this allow once every module has its caller.

mod cli;
mod draw;
mod edges;
mod image;
mod lock;
mod text;
mod units;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::parse(std::env::args().skip(1)) {
        Ok(command) => {
            eprintln!("melibea-ruler: {command:?} is not wired up yet");
            ExitCode::FAILURE
        }
        Err(message) => {
            eprintln!("melibea-ruler: {message}\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}
