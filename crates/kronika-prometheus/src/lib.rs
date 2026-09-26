//! Metric catalog, scheduling and Prometheus text exposition for the
//! collector `/metrics` endpoint.
//!
//! The catalog is the pgwatch `metrics.yaml` file format: named metrics with
//! per-PostgreSQL-major-version SQL, gauge column lists, presets mapping
//! metric names to collection intervals. The embedded copy of the pgwatch
//! v5.3.0 catalog and user overlay files load through the same
//! [`catalog::Catalog::from_yaml_str`] parser.
//!
//! Rows are converted and exposed with pgwatch Prometheus-sink compatibility:
//! family names `pgwatch_<metric>_<column>`, `dbname` plus `tag_*` labels,
//! counters unless the column is a gauge, one timestamp per fetch taken from
//! the first row's `epoch_ns`.

pub mod cache;
pub mod catalog;
pub mod engine;
pub mod executor;
pub mod expose;
pub mod measurement;
pub mod schedule;
pub mod typing;

pub use cache::{DbCache, SnapshotRow};
pub use catalog::{Catalog, EMBEDDED_CATALOG_YAML, Gauges, MetricDef, NodeStatus, PresetDef, Sql};
pub use expose::{Sample, expose_samples};
pub use measurement::{Column, QueryResult, SampleSet};
pub use schedule::{due, stale_threshold_ms};
pub use typing::{Cell, ColumnKind};
