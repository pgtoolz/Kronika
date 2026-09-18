//! Standalone Kronika HTML report generator.

mod cli;
mod help;

#[cfg(test)]
use kronika_store as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = match cli::parse_from(std::env::args_os()) {
        Ok(args) => args,
        Err(error) => {
            drop(error.print());
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1));
        }
    };
    match cli::generate(&args.input, &args.output, args.visible_range) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kronika-report: {error}");
            ExitCode::FAILURE
        }
    }
}
