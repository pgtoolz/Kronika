//! Standalone Kronika HTML report generator.

mod cli;
mod help;

use std::process::ExitCode;
use {
    base64 as _, flate2 as _, kronika_format as _, kronika_index as _, kronika_layout as _,
    kronika_query as _, kronika_reader as _, kronika_store as _, tempfile as _,
};
#[cfg(test)]
use {kronika_registry as _, kronika_writer as _, serde_json as _};

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
