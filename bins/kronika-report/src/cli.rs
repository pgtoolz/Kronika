//! Filesystem adapter for the standalone report command.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use kronika_report::{
    HtmlReportError, ReportTimeRange, write_html_from_file, write_html_from_file_with_range,
};

const TEMP_PREFIX: &str = ".kronika-report-";

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

impl From<HtmlReportError> for GenerateError {
    fn from(source: HtmlReportError) -> Self {
        Self::Document(source)
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
    let file = File::open(input).map_err(|source| GenerateError::Input {
        path: input.to_path_buf(),
        source,
    })?;
    let len = file
        .metadata()
        .map_err(|source| GenerateError::Input {
            path: input.to_path_buf(),
            source,
        })?
        .len();
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(|source| output_error(output, source))?;
    {
        let mut buffered = BufWriter::new(&mut temporary);
        match visible_range {
            Some(range) => write_html_from_file_with_range(file, len, range, &mut buffered),
            None => write_html_from_file(file, len, &mut buffered),
        }
        .map_err(|error| document_error(output, error))?;
        buffered
            .flush()
            .map_err(|source| output_error(output, source))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| output_error(output, source))?;
    temporary
        .persist(output)
        .map_err(|error| output_error(output, error.error))?;
    Ok(())
}

fn document_error(path: &Path, error: HtmlReportError) -> GenerateError {
    match error {
        HtmlReportError::Write(source) => output_error(path, source),
        source => GenerateError::Document(source),
    }
}

fn output_error(path: &Path, source: io::Error) -> GenerateError {
    GenerateError::Output {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
