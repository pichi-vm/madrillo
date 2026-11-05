#![forbid(unsafe_code)]

use clap::Parser;
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(version, about = "Native process monitor (under construction)")]
struct Cli {
    /// Print the shared protocol version.
    #[arg(long)]
    api_version: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.api_version {
        return match writeln!(io::stdout().lock(), "{}", madrillo::PROTOCOL_VERSION) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    let _ = writeln!(
        io::stderr().lock(),
        "madrillo: monitor implementation is in progress; see PLAN.md"
    );
    ExitCode::FAILURE
}
