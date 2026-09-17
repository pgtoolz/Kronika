//! Command arguments and atomic publication of a standalone report.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use clap::{CommandFactory as _, Parser};
use kronika_report::{
    HtmlReportError, ReportTimeRange, write_html_from_file, write_html_from_file_with_range,
};

const TEMP_PREFIX: &str = ".kronika-report-";

/// Turn a finished ZMS recording into one interactive HTML file.
#[derive(Debug, Parser)]
#[command(name = "kronika-report", version, after_long_help = crate::help::EXAMPLES)]
struct Args {
    /// Finished standalone ZMS file; directories and active.wal are not accepted.
    #[arg(value_name = "INPUT.zms")]
    input: PathBuf,
    /// Exact .html path; atomically replaces an existing file. Parent must exist.
    #[arg(value_name = "OUTPUT.html")]
    output: PathBuf,
    /// Inclusive visible start in Unix microseconds; requires --to-exclusive.
    #[arg(long, requires = "to_exclusive", value_name = "MICROSECONDS")]
    from: Option<i64>,
    /// Exclusive visible end in Unix microseconds; requires --from.
    #[arg(long, requires = "from", value_name = "MICROSECONDS")]
    to_exclusive: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) input: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) visible_range: Option<ReportTimeRange>,
}

/// Parse arguments and validate the visible interval before file access.
pub(crate) fn parse_from(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
) -> Result<Config, clap::Error> {
    let args = Args::try_parse_from(args)?;
    let visible_range = args
        .from
        .zip(args.to_exclusive)
        .map(|(from, to)| {
            ReportTimeRange::new(from, to).ok_or_else(|| {
                Args::command().error(
                    clap::error::ErrorKind::ValueValidation,
                    "report bounds must satisfy 0 < from < to-exclusive <= 9007199254740991 (Unix microseconds)",
                )
            })
        })
        .transpose()?;
    Ok(Config {
        input: args.input,
        output: args.output,
        visible_range,
    })
}

/// Failure while reading paths or atomically publishing an HTML document.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum GenerateError {
    /// The output does not have the `.html` suffix.
    InvalidOutputName(PathBuf),
    /// The input path could not be read.
    Input {
        /// Input path that failed.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// The reusable document writer rejected the input.
    Document(HtmlReportError),
    /// The destination temporary or final path could not be written.
    Output {
        /// Destination path that failed.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
}

impl std::fmt::Display for GenerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOutputName(path) => write!(f, "{} is not an .html path", path.display()),
            Self::Input { path, source } => write!(f, "read {}: {source}", path.display()),
            Self::Document(source) => source.fmt(f),
            Self::Output { path, source } => write!(f, "write {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for GenerateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Input { source, .. } | Self::Output { source, .. } => Some(source),
            Self::Document(source) => Some(source),
            Self::InvalidOutputName(_) => None,
        }
    }
}

/// Generate and atomically replace one standalone report.
pub(crate) fn generate(
    input: &Path,
    output: &Path,
    visible_range: Option<ReportTimeRange>,
) -> Result<(), GenerateError> {
    if output.extension() != Some(OsStr::new("html")) {
        return Err(GenerateError::InvalidOutputName(output.to_path_buf()));
    }
    let input_error = |source| GenerateError::Input {
        path: input.to_path_buf(),
        source,
    };
    let output_error = |source| GenerateError::Output {
        path: output.to_path_buf(),
        source,
    };
    let file = File::open(input).map_err(input_error)?;
    let len = file.metadata().map_err(input_error)?.len();
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(output_error)?;
    {
        let mut buffered = BufWriter::new(&mut temporary);
        match visible_range {
            Some(range) => write_html_from_file_with_range(file, len, range, &mut buffered),
            None => write_html_from_file(file, len, &mut buffered),
        }
        .map_err(|error| match error {
            HtmlReportError::Write(source) => output_error(source),
            source => GenerateError::Document(source),
        })?;
        buffered.flush().map_err(output_error)?;
    }
    temporary.as_file().sync_all().map_err(output_error)?;
    temporary
        .persist(output)
        .map_err(|error| output_error(error.error))?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/output.rs"]
mod output_tests;

#[cfg(test)]
#[path = "tests/arguments.rs"]
mod arguments_tests;
