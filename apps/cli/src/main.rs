#![forbid(unsafe_code)]
mod actions;
mod args;
mod commands;
mod elevation;
mod history;
mod inspection;
mod investigation;
mod local;
mod model_routing;
mod provider;
mod report;
mod schedule;
mod storage;
mod storage_view;
#[cfg(test)]
mod tests;
mod text;

use args::Args;
use clap::Parser;
use std::{error::Error, process::ExitCode};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    let args = Args::parse();
    text::set_language(args.language);
    if pcagent_inspect::elevation::ensure_unelevated_host().is_err() {
        eprintln!("{}", text::label("cli.standard_user"));
        return ExitCode::from(2);
    }
    match commands::run(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Error: {}", text::clean(&error.to_string()));
            ExitCode::from(2)
        }
    }
}
