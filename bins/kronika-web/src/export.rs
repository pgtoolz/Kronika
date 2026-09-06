//! Authenticated construction and bounded delivery of standalone HTML exports.

use std::fs::File;
use std::io::{self, BufWriter, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use http_body_util::BodyExt as _;
use hyper::Response;
use hyper::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue, VARY};
use kronika_dump::{SliceError, SliceRange, UtcSecond, slice_to_zms};
use kronika_reader::{Reader, ReaderError};
use kronika_report::{HtmlReportError, ReportTimeRange, write_html_from_file_with_segment_id};
use tokio::sync::{Semaphore, mpsc};

use crate::api::CachePolicy;
use crate::body::{BodyError, BodyItem, ChannelBody, ChunkWriter};
use crate::route::{MAX_QUERY_BYTES, RouteError};
use crate::{WebBody, common_headers, refused};

const FILE_BUFFER_BYTES: usize = 64 * 1_024;

pub(crate) fn parse(query: &str) -> Result<SliceRange, RouteError> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(RouteError::BadParameter("query".to_owned()));
    }
    let mut from = None;
    let mut to = None;
    if !query.is_empty() {
        for part in query.split('&') {
            if part.is_empty() {
                return Err(RouteError::BadParameter("query".to_owned()));
            }
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            match name {
                "from" if from.is_none() => from = Some(second("from", value)?),
                "to" if to.is_none() => to = Some(second("to", value)?),
                _ => return Err(RouteError::BadParameter(name.to_owned())),
            }
        }
    }
    let from = from.ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to = to.ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    let range =
        SliceRange::new(from, to).map_err(|_error| RouteError::BadParameter("from".to_owned()))?;
    validate_report_range(&range)?;
    Ok(range)
}

fn second(name: &str, value: &str) -> Result<UtcSecond, RouteError> {
    let value = value
        .parse::<i64>()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
    UtcSecond::from_unix_seconds(value).map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn validate_report_range(range: &SliceRange) -> Result<(), RouteError> {
    let from = range
        .from()
        .unix_seconds()
        .checked_mul(1_000_000)
        .filter(|value| *value > 0)
        .ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to_exclusive = range
        .to()
        .unix_seconds()
        .checked_add(1)
        .and_then(|value| value.checked_mul(1_000_000))
        .ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    ReportTimeRange::new(from, to_exclusive)
        .ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    Ok(())
}

#[derive(Debug)]
struct PreparedExport {
    file: File,
    len: u64,
    filename: String,
}

#[derive(Clone, Copy)]
struct ExportPreparation {
    requested_from: i64,
    requested_to_exclusive: i64,
    rows: u64,
    sections: usize,
    zms_bytes: u64,
    html_bytes: u64,
    open: Duration,
    slice: Duration,
    report: Duration,
    total: Duration,
}

impl std::fmt::Display for ExportPreparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "requested_from={} requested_to_exclusive={} rows={} sections={} zms_bytes={} html_bytes={} open_us={} slice_us={} report_us={} total_us={}",
            self.requested_from,
            self.requested_to_exclusive,
            self.rows,
            self.sections,
            self.zms_bytes,
            self.html_bytes,
            self.open.as_micros(),
            self.slice.as_micros(),
            self.report.as_micros(),
            self.total.as_micros(),
        )
    }
}

#[derive(Debug)]
enum ExportError {
    Reader(ReaderError),
    Slice(SliceError),
    Report(HtmlReportError),
    Temporary(io::Error),
    InvalidTime,
    InvalidHeader(hyper::header::InvalidHeaderValue),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reader(error) => write!(f, "open recorded data: {error}"),
            Self::Slice(error) => write!(f, "build standalone ZMS: {error}"),
            Self::Report(error) => write!(f, "build standalone HTML: {error}"),
            Self::Temporary(error) => write!(f, "use temporary export file: {error}"),
            Self::InvalidTime => f.write_str("format export time"),
            Self::InvalidHeader(error) => {
                write!(f, "build export response header: {error}")
            }
        }
    }
}

impl ExportError {
    const fn status_and_code(&self) -> (hyper::StatusCode, &'static str) {
        match self {
            Self::Slice(SliceError::NoRowsInRequestedRange) => {
                (hyper::StatusCode::NOT_FOUND, "export_empty")
            }
            Self::Slice(SliceError::Range(_)) => (hyper::StatusCode::BAD_REQUEST, "bad_parameter"),
            Self::Reader(_)
            | Self::Slice(_)
            | Self::Report(_)
            | Self::Temporary(_)
            | Self::InvalidTime
            | Self::InvalidHeader(_) => (hyper::StatusCode::INTERNAL_SERVER_ERROR, "export_failed"),
        }
    }
}

pub(crate) async fn response(
    data_root: PathBuf,
    range: SliceRange,
    gate: Arc<Semaphore>,
) -> Response<WebBody> {
    response_with(gate, move || build(&data_root, range)).await
}

async fn response_with(
    gate: Arc<Semaphore>,
    prepare: impl FnOnce() -> Result<PreparedExport, ExportError> + Send + 'static,
) -> Response<WebBody> {
    let Ok(permit) = gate.try_acquire_owned() else {
        return refused(hyper::StatusCode::SERVICE_UNAVAILABLE, "export_busy", None);
    };
    match tokio::task::spawn_blocking(move || {
        // The blocking work retains admission if its caller abandons the request.
        let _permit = permit;
        prepare()
    })
    .await
    {
        Ok(Ok(prepared)) => {
            prepared_response(prepared).unwrap_or_else(|error| export_failure(&error))
        }
        Ok(Err(error)) => export_failure(&error),
        Err(error) => {
            eprintln!("kronika-web: export task failed: {error}");
            refused(
                hyper::StatusCode::INTERNAL_SERVER_ERROR,
                "export_failed",
                None,
            )
        }
    }
}

fn export_failure(error: &ExportError) -> Response<WebBody> {
    eprintln!("kronika-web: export failed: {error}");
    let (status, code) = error.status_and_code();
    refused(status, code, None)
}

fn build(data_root: &Path, range: SliceRange) -> Result<PreparedExport, ExportError> {
    build_with(data_root, range, log_prepared)
}

fn build_with(
    data_root: &Path,
    range: SliceRange,
    log: impl FnOnce(ExportPreparation),
) -> Result<PreparedExport, ExportError> {
    let total_started = Instant::now();
    let open_started = Instant::now();
    let reader = Reader::open(data_root).map_err(ExportError::Reader)?;
    let open = open_started.elapsed();

    let slice_started = Instant::now();
    let mut html = tempfile::tempfile().map_err(ExportError::Temporary)?;
    let mut zms = tempfile::tempfile().map_err(ExportError::Temporary)?;
    let summary = slice_to_zms(&reader, range, &mut html, &mut zms).map_err(ExportError::Slice)?;
    zms.flush().map_err(ExportError::Temporary)?;
    let zms_len = zms.metadata().map_err(ExportError::Temporary)?.len();
    if zms_len != summary.bytes_written {
        return Err(ExportError::Temporary(io::Error::other(
            "standalone ZMS length changed",
        )));
    }
    let slice = slice_started.elapsed();

    let report_started = Instant::now();
    html.set_len(0).map_err(ExportError::Temporary)?;
    html.seek(SeekFrom::Start(0))
        .map_err(ExportError::Temporary)?;
    {
        let mut output = BufWriter::with_capacity(FILE_BUFFER_BYTES, &mut html);
        let visible_range =
            ReportTimeRange::new(summary.requested_from, summary.requested_to_exclusive)
                .ok_or(ExportError::InvalidTime)?;
        write_html_from_file_with_segment_id(
            summary.segment_id,
            zms,
            summary.bytes_written,
            visible_range,
            &mut output,
        )
        .map_err(ExportError::Report)?;
        output.flush().map_err(ExportError::Temporary)?;
    }
    let len = html.metadata().map_err(ExportError::Temporary)?.len();
    html.seek(SeekFrom::Start(0))
        .map_err(ExportError::Temporary)?;
    let prepared = PreparedExport {
        file: html,
        len,
        filename: filename(range).ok_or(ExportError::InvalidTime)?,
    };
    let report = report_started.elapsed();
    let total = total_started.elapsed();
    log(ExportPreparation {
        requested_from: summary.requested_from,
        requested_to_exclusive: summary.requested_to_exclusive,
        rows: summary.rows_written,
        sections: summary.sections_written,
        zms_bytes: summary.bytes_written,
        html_bytes: len,
        open,
        slice,
        report,
        total,
    });
    Ok(prepared)
}

fn log_prepared(preparation: ExportPreparation) {
    eprintln!("kronika-web: export_prepared {preparation}");
}

fn filename(range: SliceRange) -> Option<String> {
    let from = utc_filename_second(range.from())?;
    let to = utc_filename_second(range.to())?;
    Some(format!("kronika-{from}-{to}-utc.html"))
}

fn utc_filename_second(value: UtcSecond) -> Option<String> {
    DateTime::<Utc>::from_timestamp(value.unix_seconds(), 0)
        .map(|time| time.format("%Y-%m-%d-%H%M%S").to_string())
}

fn prepared_response(prepared: PreparedExport) -> Result<Response<WebBody>, ExportError> {
    let PreparedExport {
        mut file,
        len,
        filename,
    } = prepared;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(ExportError::InvalidHeader)?;
    let (sender, receiver) = mpsc::channel::<BodyItem>(8);
    let handle = tokio::task::spawn_blocking(move || stream_file(&mut file, len, &sender));
    drop(handle);

    let body = ChannelBody { receiver }.boxed_unsync();
    let mut response = Response::new(body);
    common_headers(&mut response, CachePolicy::NoStore);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(len));
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, disposition);
    Ok(response)
}

fn stream_file(file: &mut File, len: u64, sender: &mpsc::Sender<BodyItem>) {
    let mut output = ChunkWriter::new(sender.clone());
    let copied = io::copy(&mut file.take(len), &mut output);
    let finished = output.finish();
    let result = copied.and_then(|copied| {
        let remaining = len.saturating_sub(copied);
        finished?;
        if remaining == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("generated export ended {remaining} bytes before declared length"),
            ))
        }
    });
    if let Err(error) = result
        && !sender.is_closed()
    {
        eprintln!("kronika-web: stream generated export: {error}");
        let _sent = sender.blocking_send(Err(BodyError));
    }
}

#[cfg(test)]
#[path = "export/tests.rs"]
mod tests;
