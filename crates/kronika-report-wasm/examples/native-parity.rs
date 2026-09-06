//! Native byte oracle for the browser adapter harness.

use std::io::Write as _;

use kronika_layout::SegmentId;
use kronika_query::{QuerySink, SOURCE_OS, SOURCE_POSTGRESQL};
use kronika_report::{ReportEngine, ReportInput};
use kronika_report_wasm as _;
use wasm_bindgen as _;

const SEGMENT_ID: &str = "1709164800000000";
const ZMS: &[u8] = include_bytes!("../../../bins/kronika-report/tests/fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("../../../bins/kronika-report/tests/fixtures/standalone.idx");

fn argument(position: usize, name: &str) -> Result<String, std::io::Error> {
    std::env::args()
        .nth(position)
        .ok_or_else(|| std::io::Error::other(format!("missing {name} argument")))
}

#[derive(Debug, Default)]
struct Records(Vec<u8>);

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0.extend_from_slice(&bytes);
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = argument(1, "path")?;
    let query = argument(2, "query")?;
    let request = kronika_api::parse(&path, Some(&query))
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .into_query()?;
    let engine = ReportEngine::new(ReportInput {
        segment_id: SegmentId::new(SEGMENT_ID.parse()?)?,
        zms: ZMS.to_vec(),
        idx: IDX.to_vec(),
        configured_sources: SOURCE_OS | SOURCE_POSTGRESQL,
        max_zms_bytes: u64::try_from(ZMS.len())?,
    })?;
    let mut records = Records::default();
    engine.execute(request, &mut records)?;
    std::io::stdout().lock().write_all(&records.0)?;
    Ok(())
}
