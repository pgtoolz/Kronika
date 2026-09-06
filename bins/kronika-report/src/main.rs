//! Standalone Kronika HTML report generator.

mod cli;
mod help;

use kronika_report::ReportTimeRange;
#[cfg(test)]
use serde_json as _;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use {
    base64 as _, flate2 as _, kronika_format as _, kronika_index as _, kronika_layout as _,
    kronika_query as _, kronika_reader as _, kronika_store as _, tempfile as _,
};

const USAGE: &str = "usage: kronika-report [--from <unix_microseconds> --to-exclusive <unix_microseconds>] <input>.zms <output>.html";

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    input: PathBuf,
    output: PathBuf,
    visible_range: Option<ReportTimeRange>,
}

fn arguments(values: impl IntoIterator<Item = OsString>) -> Result<Arguments, &'static str> {
    let values = values.into_iter().collect::<Vec<_>>();
    let (input, output, visible_range) = match values.as_slice() {
        [input, output] => (input, output, None),
        [from_flag, from, to_flag, to, input, output]
            if from_flag == "--from" && to_flag == "--to-exclusive" =>
        {
            let from = from
                .to_str()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or("invalid --from Unix microseconds")?;
            let to_exclusive = to
                .to_str()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or("invalid --to-exclusive Unix microseconds")?;
            let range = ReportTimeRange::new(from, to_exclusive)
                .ok_or("report range must use positive JavaScript-safe Unix microseconds")?;
            (input, output, Some(range))
        }
        [] => return Err("missing standalone ZMS input"),
        [_] => return Err("missing HTML output"),
        _ => return Err("expected one ZMS input and one HTML output"),
    };
    if input.as_encoded_bytes().starts_with(b"-") || output.as_encoded_bytes().starts_with(b"-") {
        return Err("unknown or misplaced option; use ./ before a path beginning with '-'");
    }
    Ok(Arguments {
        input: PathBuf::from(input.as_os_str()),
        output: PathBuf::from(output.as_os_str()),
        visible_range,
    })
}

fn main() -> ExitCode {
    if std::env::args_os().skip(1).eq(["--version"]) {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if std::env::args_os().skip(1).eq(["--help"]) || std::env::args_os().skip(1).eq(["-h"]) {
        print!("{}", help::HELP);
        return ExitCode::SUCCESS;
    }
    let arguments = match arguments(std::env::args_os().skip(1)) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("kronika-report: {error}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match cli::generate(&arguments.input, &arguments.output, arguments.visible_range) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kronika-report: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
