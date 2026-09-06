//! Assembly of one self-contained report document.

use std::fs::File;
use std::io::{self, Read as _, Write as _};

use base64::engine::general_purpose::STANDARD;
use base64::write::EncoderWriter;
use flate2::read::GzDecoder;
use kronika_format::ReadAt;
use kronika_layout::{LayoutError, SegmentId};
use kronika_query::{SOURCE_OS, source_bit};
use kronika_reader::{FinishedReader, ReaderError};
use kronika_store::{
    EmbeddedResource, EmbeddedSource, ImmutableSegmentSource as _, ResourceError, SegmentResource,
    read_resource_catalog,
};

const SHELL_GZIP: &[u8] = include_bytes!("../assets/kronika-report-shell.html.gz");
const WASM_GLUE: &[u8] = include_bytes!("../assets/kronika-report-wasm.js");
#[allow(
    clippy::large_include_file,
    reason = "the generator embeds the complete compressed query module in its HTML output"
)]
const WASM_GZIP: &[u8] = include_bytes!("../assets/kronika-report-wasm.wasm.gz");
const RUNTIME_MARKER: &[u8] = b"/*KRONIKA_REPORT_RUNTIME*/";
const RUNTIME_START: &[u8] = br#";(()=>{const b=s=>Uint8Array.from(atob(s),c=>c.charCodeAt(0));globalThis.__KRONIKA_REPORT_RUNTIME__={visibleFrom:""#;
const RUNTIME_TO: &[u8] = br#"",visibleToExclusive:""#;
const RUNTIME_READY: &[u8] = br#"",ready:(async()=>{const z=b(""#;
const RUNTIME_INDEX: &[u8] = br#""),i=b(""#;
const RUNTIME_WASM: &[u8] = br#""),g=b(""#;
const RUNTIME_ID: &[u8] = br#"");const r=new Uint8Array(await new Response(new Blob([g]).stream().pipeThrough(new DecompressionStream("gzip"))).arrayBuffer()),m=await WebAssembly.compile(r);await KronikaReportWasm.initEmbedded(m);return new KronikaReportWasm.ReportSession(""#;
const RUNTIME_SOURCES: &[u8] = br#"",z,i,"#;
const RUNTIME_LENGTH: &[u8] = br#",BigInt(""#;
const RUNTIME_END: &[u8] = br#""));})()};})();"#;
const BASE64_INPUT_BYTES: usize = 12 * 1024;
const MAX_JAVASCRIPT_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Owned input for one self-contained HTML document.
#[derive(Debug)]
pub struct HtmlReportInput {
    /// Explicit identity bound to the embedded ZMS and derived IDX.
    pub segment_id: SegmentId,
    /// Complete finished ZMS allocation.
    pub zms: Vec<u8>,
    /// Maximum accepted logical ZMS length in bytes.
    pub max_zms_bytes: u64,
    /// Exact half-open time range exposed by the report interface.
    pub visible_range: ReportTimeRange,
}

/// Exact half-open time range exposed by one report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportTimeRange {
    from: i64,
    to_exclusive: i64,
}

impl ReportTimeRange {
    /// Build a positive, JavaScript-safe, non-empty half-open time range.
    #[must_use]
    pub const fn new(from: i64, to_exclusive: i64) -> Option<Self> {
        if from > 0 && from < to_exclusive && to_exclusive <= MAX_JAVASCRIPT_SAFE_INTEGER {
            Some(Self { from, to_exclusive })
        } else {
            None
        }
    }

    /// Inclusive lower bound in Unix microseconds.
    #[must_use]
    pub const fn from(self) -> i64 {
        self.from
    }

    /// Exclusive upper bound in Unix microseconds.
    #[must_use]
    pub const fn to_exclusive(self) -> i64 {
        self.to_exclusive
    }
}

/// Facts about one successfully written HTML document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HtmlReportSummary {
    /// Identity bound to both embedded artifacts.
    pub segment_id: SegmentId,
    /// Logical ZMS length in bytes.
    pub zms_bytes: u64,
    /// Canonical IDX length in bytes.
    pub idx_bytes: u64,
    /// Source-family bits embedded for report queries.
    pub configured_sources: u32,
}

/// Failure while assembling a self-contained HTML document.
#[derive(Debug)]
#[non_exhaustive]
pub enum HtmlReportError {
    /// An in-memory artifact length cannot be represented by the report ABI.
    InputTooLarge(usize),
    /// The report-visible time range is empty or cannot be represented.
    InvalidTimeRange,
    /// The validated catalog timestamp is outside the segment-id domain.
    Layout(LayoutError),
    /// Invalid or over-limit finished ZMS bytes.
    Resource(ResourceError),
    /// The production reader rejected the ZMS.
    Reader(ReaderError),
    /// The production index builder rejected decoded rows.
    Build(kronika_index::BuildError),
    /// The canonical index encoder rejected the derived index.
    Index(kronika_index::IndexError),
    /// The embedded report shell could not be decoded.
    Asset(io::Error),
    /// The embedded report shell has an invalid runtime marker.
    InvalidAsset(&'static str),
    /// The embedded source did not expose exactly one ZMS.
    InvalidResourceCount(usize),
    /// The caller-owned output sink rejected bytes.
    Write(io::Error),
    /// Positional input bytes could not be read.
    Read(io::Error),
}

impl std::fmt::Display for HtmlReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge(bytes) => {
                write!(f, "a {bytes}-byte ZMS is too large for the report ABI")
            }
            Self::InvalidTimeRange => f.write_str(
                "report-visible bounds must be positive JavaScript-safe Unix microseconds",
            ),
            Self::Layout(source) => source.fmt(f),
            Self::Resource(source) => source.fmt(f),
            Self::Reader(source) => source.fmt(f),
            Self::Build(source) => source.fmt(f),
            Self::Index(source) => source.fmt(f),
            Self::Asset(source) => write!(f, "read the embedded report shell: {source}"),
            Self::InvalidAsset(message) => {
                write!(f, "invalid embedded report asset: {message}")
            }
            Self::InvalidResourceCount(count) => {
                write!(f, "the embedded ZMS produced {count} resources")
            }
            Self::Write(source) => write!(f, "write HTML: {source}"),
            Self::Read(source) => write!(f, "read ZMS: {source}"),
        }
    }
}

impl std::error::Error for HtmlReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Layout(source) => Some(source),
            Self::Resource(source) => Some(source),
            Self::Reader(source) => Some(source),
            Self::Build(source) => Some(source),
            Self::Index(source) => Some(source),
            Self::Asset(source) | Self::Write(source) | Self::Read(source) => Some(source),
            Self::InputTooLarge(_)
            | Self::InvalidTimeRange
            | Self::InvalidAsset(_)
            | Self::InvalidResourceCount(_) => None,
        }
    }
}

impl From<LayoutError> for HtmlReportError {
    fn from(source: LayoutError) -> Self {
        Self::Layout(source)
    }
}

impl From<ResourceError> for HtmlReportError {
    fn from(source: ResourceError) -> Self {
        Self::Resource(source)
    }
}

impl From<ReaderError> for HtmlReportError {
    fn from(source: ReaderError) -> Self {
        Self::Reader(source)
    }
}

impl From<kronika_index::BuildError> for HtmlReportError {
    fn from(source: kronika_index::BuildError) -> Self {
        Self::Build(source)
    }
}

impl From<kronika_index::IndexError> for HtmlReportError {
    fn from(source: kronika_index::IndexError) -> Self {
        Self::Index(source)
    }
}

/// Write one self-contained HTML document to a caller-owned sink.
///
/// The ZMS allocation moves once behind shared ownership. The same allocation
/// feeds the base64 encoder and the production reader, so the operation does
/// not retain a second complete ZMS allocation.
///
/// # Errors
///
/// Returns a typed input, reader, index, asset, or output-sink failure. The
/// caller decides how to discard a partially written sink after an error.
pub fn write_html(
    input: HtmlReportInput,
    output: &mut dyn io::Write,
) -> Result<HtmlReportSummary, HtmlReportError> {
    let HtmlReportInput {
        segment_id,
        zms,
        max_zms_bytes,
        visible_range,
    } = input;
    let source = EmbeddedSource::from_owned(segment_id, zms, max_zms_bytes)?;
    write_source_html(segment_id, &source, None, Some(visible_range), output)
}

/// Write one self-contained HTML document from an already-open ZMS file.
///
/// # Errors
///
/// Returns a typed input, reader, index, asset, or output-sink failure. The
/// complete file is validated and its identity is derived from catalog
/// `min_ts` before the first output byte is written.
pub fn write_html_from_file(
    file: File,
    max_zms_bytes: u64,
    output: &mut dyn io::Write,
) -> Result<HtmlReportSummary, HtmlReportError> {
    write_file_html(file, max_zms_bytes, None, output)
}

/// Write one self-contained HTML document with an explicit visible range.
///
/// The segment identity is still derived from the validated ZMS catalog.
///
/// # Errors
///
/// Returns a typed input, reader, index, asset, or output-sink failure.
pub fn write_html_from_file_with_range(
    file: File,
    max_zms_bytes: u64,
    visible_range: ReportTimeRange,
    output: &mut dyn io::Write,
) -> Result<HtmlReportSummary, HtmlReportError> {
    write_file_html(file, max_zms_bytes, Some(visible_range), output)
}

fn write_file_html(
    file: File,
    max_zms_bytes: u64,
    visible_range: Option<ReportTimeRange>,
    output: &mut dyn io::Write,
) -> Result<HtmlReportSummary, HtmlReportError> {
    let len = file.byte_len().map_err(HtmlReportError::Read)?;
    if len > max_zms_bytes {
        return Err(ResourceError::TooLarge {
            len,
            max: max_zms_bytes,
        }
        .into());
    }
    let min_ts = read_resource_catalog(&file)?.min_ts;
    let segment_id = SegmentId::new(min_ts)?;
    let source = EmbeddedSource::from_file(segment_id, file, max_zms_bytes)?;
    write_source_html(segment_id, &source, Some(min_ts), visible_range, output)
}

/// Write one self-contained HTML document from an already-open ZMS file under
/// the supplied segment identity.
///
/// This entry point is for callers that already own the identity associated
/// with generated ZMS bytes. Unlike [`write_html_from_file`], it does not
/// derive that identity from the earliest row in the file.
///
/// # Errors
///
/// Returns a typed input, reader, index, asset, or output-sink failure. The
/// complete file is validated before the first output byte is written.
pub fn write_html_from_file_with_segment_id(
    segment_id: SegmentId,
    file: File,
    max_zms_bytes: u64,
    visible_range: ReportTimeRange,
    output: &mut dyn io::Write,
) -> Result<HtmlReportSummary, HtmlReportError> {
    let source = EmbeddedSource::from_file(segment_id, file, max_zms_bytes)?;
    write_source_html(segment_id, &source, None, Some(visible_range), output)
}

fn write_source_html(
    segment_id: SegmentId,
    source: &EmbeddedSource,
    expected_min_ts: Option<i64>,
    visible_range: Option<ReportTimeRange>,
    output: &mut dyn io::Write,
) -> Result<HtmlReportSummary, HtmlReportError> {
    let reader = FinishedReader::new(source.clone());
    let listing = reader.resources()?;
    let [resource] = listing.resources.as_slice() else {
        return Err(HtmlReportError::InvalidResourceCount(
            listing.resources.len(),
        ));
    };
    if expected_min_ts.is_some_and(|min_ts| min_ts != resource.summary().min_ts) {
        return Err(ResourceError::Changed.into());
    }
    let visible_range = match visible_range {
        Some(range) => range,
        None => ReportTimeRange::new(
            resource.summary().min_ts,
            resource
                .summary()
                .max_ts
                .checked_add(1)
                .ok_or(HtmlReportError::InvalidTimeRange)?,
        )
        .ok_or(HtmlReportError::InvalidTimeRange)?,
    };
    let zms_len = resource.captured_bytes();
    let bytes = source.open_resource(resource)?;

    let (idx, configured_sources) = isolated_index(&reader, resource)?;
    source.validate_opened(resource, &bytes)?;
    let shell = shell()?;
    let marker = marker(&shell)?;

    write_output(output, &shell[..marker])?;
    write_output(output, WASM_GLUE)?;
    write_output(output, RUNTIME_START)?;
    write!(output, "{}", visible_range.from()).map_err(HtmlReportError::Write)?;
    write_output(output, RUNTIME_TO)?;
    write!(output, "{}", visible_range.to_exclusive()).map_err(HtmlReportError::Write)?;
    write_output(output, RUNTIME_READY)?;
    write_base64_reader(output, &bytes, zms_len)?;
    write_output(output, RUNTIME_INDEX)?;

    let idx_len =
        u64::try_from(idx.len()).map_err(|_overflow| HtmlReportError::InputTooLarge(idx.len()))?;
    write_base64(output, &idx)?;
    write_output(output, RUNTIME_WASM)?;
    write_base64(output, WASM_GZIP)?;
    write_output(output, RUNTIME_ID)?;
    write!(output, "{segment_id}").map_err(HtmlReportError::Write)?;
    write_output(output, RUNTIME_SOURCES)?;
    write!(output, "{configured_sources}").map_err(HtmlReportError::Write)?;
    write_output(output, RUNTIME_LENGTH)?;
    write!(output, "{zms_len}").map_err(HtmlReportError::Write)?;
    write_output(output, RUNTIME_END)?;
    write_output(output, &shell[marker + RUNTIME_MARKER.len()..])?;
    source.validate_opened(resource, &bytes)?;

    Ok(HtmlReportSummary {
        segment_id,
        zms_bytes: zms_len,
        idx_bytes: idx_len,
        configured_sources,
    })
}

fn isolated_index(
    reader: &FinishedReader<EmbeddedSource>,
    resource: &SegmentResource<EmbeddedResource>,
) -> Result<(Vec<u8>, u32), HtmlReportError> {
    let segment = reader.open_segment(resource)?;
    let configured_sources = configured_sources(segment.type_ids());
    let index = kronika_index::build(&segment)?;
    Ok((index.encode()?, configured_sources))
}

fn configured_sources(type_ids: impl IntoIterator<Item = u32>) -> u32 {
    type_ids
        .into_iter()
        .filter_map(source_bit)
        .fold(SOURCE_OS, |sources, source| sources | source)
}

fn shell() -> Result<Vec<u8>, HtmlReportError> {
    let mut shell = Vec::new();
    GzDecoder::new(SHELL_GZIP)
        .read_to_end(&mut shell)
        .map_err(HtmlReportError::Asset)?;
    Ok(shell)
}

fn marker(shell: &[u8]) -> Result<usize, HtmlReportError> {
    let mut markers = shell
        .windows(RUNTIME_MARKER.len())
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == RUNTIME_MARKER).then_some(offset));
    let Some(marker) = markers.next() else {
        return Err(HtmlReportError::InvalidAsset(
            "the runtime marker is absent",
        ));
    };
    if markers.next().is_some() {
        return Err(HtmlReportError::InvalidAsset(
            "the runtime marker occurs more than once",
        ));
    }
    Ok(marker)
}

fn write_base64(output: &mut dyn io::Write, bytes: &[u8]) -> Result<(), HtmlReportError> {
    let mut encoder = EncoderWriter::new(output, &STANDARD);
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish().map(|_output| ()))
        .map_err(HtmlReportError::Write)
}

fn write_base64_reader<R: ReadAt>(
    output: &mut dyn io::Write,
    input: &R,
    len: u64,
) -> Result<(), HtmlReportError> {
    let mut encoder = EncoderWriter::new(output, &STANDARD);
    let mut buffer = [0_u8; BASE64_INPUT_BYTES];
    let mut offset = 0_u64;
    while offset < len {
        let remaining = len - offset;
        let chunk_len =
            usize::try_from(remaining.min(BASE64_INPUT_BYTES as u64)).map_err(|_overflow| {
                HtmlReportError::Read(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "base64 input chunk does not fit memory",
                ))
            })?;
        input
            .read_exact_at(&mut buffer[..chunk_len], offset)
            .map_err(HtmlReportError::Read)?;
        encoder
            .write_all(&buffer[..chunk_len])
            .map_err(HtmlReportError::Write)?;
        offset += chunk_len as u64;
    }
    encoder
        .finish()
        .map(|_output| ())
        .map_err(HtmlReportError::Write)
}

fn write_output(output: &mut dyn io::Write, bytes: &[u8]) -> Result<(), HtmlReportError> {
    output.write_all(bytes).map_err(HtmlReportError::Write)
}

#[cfg(test)]
#[path = "generator_tests.rs"]
mod tests;
