//! Inspect Kronika storage or extract one bounded standalone ZMS.

mod args;
mod help;
mod render;

use std::fmt;
use std::fs::File;
use std::io;
use std::io::Write as _;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kronika_reader::{Reader, ReaderError};
use kronika_slice::{SliceError, SliceRange, slice_to_zms};
use kronika_store::{ResourceError, validate_finished_zms};

use crate::args::{Command, View};

fn main() -> ExitCode {
    let parsed = match args::parse(std::env::args_os().skip(1)) {
        Ok(parsed) => parsed,
        Err(problem) => {
            let success = !problem.use_stderr();
            let _printed = problem.print();
            return if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let result = match &parsed {
        Command::Inspect(arguments) => run_inspect(arguments, &mut output),
        Command::Slice(arguments) => run_slice(arguments, &mut output),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(DumpError::Output(problem)) if problem.kind() == io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("kronika-dump: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn run_inspect(args: &args::InspectArgs, output: &mut impl io::Write) -> Result<(), DumpError> {
    let reader = Reader::open(&args.root)?;
    let listing = reader.segments((
        args.from.map_or(Bound::Unbounded, Bound::Included),
        args.to.map_or(Bound::Unbounded, Bound::Included),
    ))?;
    for warning in &listing.warnings {
        render::warning(output, args.json, warning)?;
    }
    for reference in &listing.segments {
        let segment = reader.open_segment(reference)?;
        match args.view {
            View::Sizes => render::sizes(output, args.json, &segment)?,
            View::Index => {
                render::index(output, args.json, &reader, reference, &segment)?;
            }
            View::Section(type_id) => {
                render::section(output, args.json, &segment, type_id, args.limit)?;
            }
        }
    }
    Ok(())
}

fn run_slice(args: &args::SliceArgs, output: &mut impl io::Write) -> Result<(), DumpError> {
    let reader = Reader::open(&args.storage_dir)?;
    let parent = args
        .out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if args.out.try_exists()? {
        return Err(DumpError::OutputExists(args.out.clone()));
    }
    let mut scratch = tempfile::tempfile_in(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let range = SliceRange::new(args.from, args.to).map_err(SliceError::from)?;
    let summary = slice_to_zms(&reader, range, &mut scratch, temporary.as_file_mut())?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    let catalog = validate_finished_zms(temporary.as_file(), summary.bytes_written)?;
    if catalog.min_ts != summary.actual_min_ts || catalog.max_ts != summary.actual_max_ts {
        return Err(DumpError::GeneratedBoundsMismatch);
    }
    let persisted = temporary.persist_noclobber(&args.out).map_err(|problem| {
        if problem.error.kind() == io::ErrorKind::AlreadyExists {
            DumpError::OutputExists(args.out.clone())
        } else {
            DumpError::Output(problem.error)
        }
    })?;
    persisted.sync_all()?;
    File::open(parent)?.sync_all()?;
    writeln!(
        output,
        "wrote={} bytes={} rows={} sections={} segment_id={} requested_from={} requested_to_exclusive={} actual_min_ts={} actual_max_ts={}",
        args.out.display(),
        summary.bytes_written,
        summary.rows_written,
        summary.sections_written,
        summary.segment_id,
        summary.requested_from,
        summary.requested_to_exclusive,
        summary.actual_min_ts,
        summary.actual_max_ts,
    )?;
    Ok(())
}

#[derive(Debug)]
enum DumpError {
    Build(kronika_index::BuildError),
    Index(kronika_index::IndexError),
    Reader(ReaderError),
    Slice(SliceError),
    Validation(ResourceError),
    Output(io::Error),
    OutputExists(PathBuf),
    GeneratedBoundsMismatch,
}

impl fmt::Display for DumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(problem) => problem.fmt(f),
            Self::Index(problem) => problem.fmt(f),
            Self::Reader(problem) => problem.fmt(f),
            Self::Slice(problem) => problem.fmt(f),
            Self::Validation(problem) => write!(f, "validate generated ZMS: {problem}"),
            Self::Output(problem) => write!(f, "write output: {problem}"),
            Self::OutputExists(path) => {
                write!(f, "output already exists: {}", path.display())
            }
            Self::GeneratedBoundsMismatch => {
                f.write_str("generated ZMS catalog does not match selected bounds")
            }
        }
    }
}

impl std::error::Error for DumpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(problem) => Some(problem),
            Self::Index(problem) => Some(problem),
            Self::Reader(problem) => Some(problem),
            Self::Slice(problem) => Some(problem),
            Self::Validation(problem) => Some(problem),
            Self::Output(problem) => Some(problem),
            Self::OutputExists(_) | Self::GeneratedBoundsMismatch => None,
        }
    }
}

impl From<kronika_index::BuildError> for DumpError {
    fn from(problem: kronika_index::BuildError) -> Self {
        Self::Build(problem)
    }
}

impl From<kronika_index::IndexError> for DumpError {
    fn from(problem: kronika_index::IndexError) -> Self {
        Self::Index(problem)
    }
}

impl From<ReaderError> for DumpError {
    fn from(problem: ReaderError) -> Self {
        Self::Reader(problem)
    }
}

impl From<SliceError> for DumpError {
    fn from(problem: SliceError) -> Self {
        Self::Slice(problem)
    }
}

impl From<ResourceError> for DumpError {
    fn from(problem: ResourceError) -> Self {
        Self::Validation(problem)
    }
}

impl From<io::Error> for DumpError {
    fn from(problem: io::Error) -> Self {
        Self::Output(problem)
    }
}
