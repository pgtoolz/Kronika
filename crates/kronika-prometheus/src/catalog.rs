//! Catalog model and the pgwatch `metrics.yaml` loader.
//!
//! The embedded catalog is pgwatch v5.3.0 `internal/metrics/metrics.yaml`
//! (commit `456bafb811b10b595d032739ae7bbcab98908288`, BSD-3-Clause) loaded
//! through the same public [`Catalog::from_yaml_str`] a user overlay file
//! uses. Overlays replace definitions by name instead of replacing the whole
//! catalog, so a single metric override does not require copying upstream SQL.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// The pgwatch v5.3.0 catalog, embedded verbatim.
pub const EMBEDDED_CATALOG_YAML: &str = include_str!("catalog/pgwatch-metrics-v5.3.0.yaml");

/// Expected shape of the pinned embedded catalog; loading fails loudly if a
/// catalog update changes it.
pub const EMBEDDED_METRIC_COUNT: usize = 74;
/// Preset count half of the pinned shape check.
pub const EMBEDDED_PRESET_COUNT: usize = 15;

/// Largest accepted preset interval in seconds (~285 years).
pub const MAX_INTERVAL_S: u64 = 9_000_000_000_000;

/// Name of the default preset applied when none is configured.
pub const DEFAULT_PRESET: &str = "basic";

/// Metrics the pgwatch Prometheus sink never exposes; allowed in presets and
/// silently skipped (upstream `notSupportedMetrics`).
pub const NOT_EXPOSED_METRICS: [&str; 5] = [
    "change_events",
    "pgbouncer_stats",
    "pgbouncer_clients",
    "pgpool_processes",
    "pgpool_stats",
];

/// Column whose values are gauges instead of counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gauges {
    /// `gauges: ['*']` — every value column is a gauge.
    All,
    /// Named columns are gauges; the rest are counters.
    Columns(Vec<String>),
}

impl Gauges {
    fn from_list(list: Vec<String>) -> Result<Self, CatalogError> {
        // Upstream only special-cases gauges[0] == '*' (prometheus.go); a
        // '*' anywhere else is an ordinary, never-matching column name.
        if list.first().is_some_and(|g| g == "*") {
            return Ok(Self::All);
        }
        if list.iter().any(String::is_empty) {
            return Err(CatalogError::invalid("gauges: empty column name"));
        }
        Ok(Self::Columns(list))
    }

    /// Whether `column` is a gauge.
    #[must_use]
    pub fn contains(&self, column: &str) -> bool {
        match self {
            Self::All => true,
            Self::Columns(names) => names.iter().any(|n| n == column),
        }
    }
}

/// Restricts a metric to servers in (or out of) recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Run only outside recovery.
    Primary,
    /// Run only in recovery.
    Standby,
}

/// One catalog metric definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricDef {
    /// Human-readable text; used as the exposition HELP line.
    pub description: String,
    /// SQL per minimal `PostgreSQL` major version.
    pub sqls: BTreeMap<u32, Sql>,
    /// Value columns exposed as gauges.
    pub gauges: Gauges,
    /// Collected once per interval for the whole instance, not per database.
    pub is_instance_level: bool,
    /// Recovery-role restriction.
    pub node_status: Option<NodeStatus>,
    /// Per-metric server-side `statement_timeout` override in seconds.
    pub statement_timeout_seconds: Option<u64>,
    /// Exposition name override; see [`MetricDef::exposed_name`].
    pub storage_name: Option<String>,
}

/// One `sqls` entry: either runnable SQL or an explicit no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sql {
    /// A single `SELECT` / `WITH … SELECT` without a trailing semicolon.
    Select(String),
    /// Empty SQL or a semicolon/comment-only statement: run nothing for this
    /// variant (upstream `checkpointer` on v14).
    Skip,
}

impl Sql {
    /// Runnable text, or `None` for [`Sql::Skip`].
    #[must_use]
    pub fn select(&self) -> Option<&str> {
        match self {
            Self::Select(sql) => Some(sql),
            Self::Skip => None,
        }
    }
}

impl MetricDef {
    /// Name used in exposition families and HELP: `storage_name` when set,
    /// otherwise the catalog name.
    #[must_use]
    pub fn exposed_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.storage_name.as_deref().unwrap_or(name)
    }

    /// SQL for a server major version: the greatest key `<= version`, exact
    /// key preferred. `None` when the catalog has no variant for the server.
    #[must_use]
    pub fn sql_for_version(&self, version: u32) -> Option<&Sql> {
        self.sqls.range(..=version).next_back().map(|(_, sql)| sql)
    }
}

/// One preset: metric name to collection interval in seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetDef {
    /// Human-readable text.
    pub description: String,
    /// Metric name to interval seconds.
    pub metrics: BTreeMap<String, u64>,
}

/// A set of metric and preset definitions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalog {
    /// Metric definitions by name.
    pub metrics: BTreeMap<String, MetricDef>,
    /// Preset definitions by name.
    pub presets: BTreeMap<String, PresetDef>,
}

/// A catalog load or validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    /// Source file or embedded catalog.
    pub source: String,
    /// Failure description with YAML location when available.
    pub message: String,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.source, self.message)
    }
}

impl std::error::Error for CatalogError {}

impl CatalogError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            source: String::new(),
            message: message.into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMetric {
    #[serde(default)]
    description: String,
    #[serde(default)]
    sqls: BTreeMap<u32, String>,
    #[serde(default)]
    gauges: Vec<String>,
    #[serde(default)]
    is_instance_level: bool,
    #[serde(default)]
    node_status: Option<String>,
    #[serde(default)]
    statement_timeout_seconds: Option<u64>,
    #[serde(default)]
    storage_name: Option<String>,
    // Accepted for pgwatch compatibility; the exporter runs no init SQL and
    // has no private-metric handling.
    #[serde(default, rename = "init_sql")]
    _init_sql: Option<String>,
    #[serde(default, rename = "is_private")]
    _is_private: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    #[serde(default)]
    description: String,
    metrics: BTreeMap<String, u64>,
}

#[derive(Deserialize)]
struct RawCatalog {
    #[serde(default)]
    metrics: BTreeMap<String, RawMetric>,
    #[serde(default)]
    presets: BTreeMap<String, RawPreset>,
}

impl Catalog {
    /// Loads and validates catalog YAML; `source` names the file in errors.
    ///
    /// # Errors
    ///
    /// Returns a [`CatalogError`] with the file and YAML line for parse and
    /// validation failures (unknown keys, invalid SQL shape, unknown
    /// `node_status`, zero timeout).
    pub fn from_yaml_str(yaml: &str, source: &str) -> Result<Self, CatalogError> {
        let err = |message: String| CatalogError {
            source: source.to_owned(),
            message,
        };
        let raw: RawCatalog = serde_yaml::from_str(yaml).map_err(|e| {
            err(format!(
                "line {}: {e}",
                e.location().map_or(0, |l| l.line())
            ))
        })?;

        let mut metrics = BTreeMap::new();
        for (name, m) in raw.metrics {
            // Upstream parses a free string; only primary/standby restrict,
            // anything else behaves unrestricted (types.go), so unknown
            // values load instead of failing.
            let node_status = match m.node_status.as_deref() {
                Some("primary") => Some(NodeStatus::Primary),
                Some("standby") => Some(NodeStatus::Standby),
                _ => None,
            };
            if m.statement_timeout_seconds == Some(0) {
                return Err(err(format!(
                    "metric {name}: statement_timeout_seconds must be > 0"
                )));
            }
            let mut sqls = BTreeMap::new();
            // The never-executed pooler metrics query the PgBouncer console
            // (`show clients`, version key 0), not PostgreSQL; their SQL is
            // kept verbatim and the engine refuses to run them by name.
            let exempt = NOT_EXPOSED_METRICS.contains(&name.as_str());
            for (version, sql) in m.sqls {
                let classified = if exempt {
                    Ok(Sql::Select(sql.trim().to_owned()))
                } else {
                    classify_sql(&sql)
                };
                sqls.insert(
                    version,
                    classified
                        .map_err(|reason| err(format!("metric {name} sqls.{version}: {reason}")))?,
                );
            }
            metrics.insert(
                name.clone(),
                MetricDef {
                    description: m.description,
                    sqls,
                    gauges: Gauges::from_list(m.gauges).map_err(|e| {
                        let CatalogError { message, .. } = e;
                        err(format!("metric {name}: {message}"))
                    })?,
                    is_instance_level: m.is_instance_level,
                    node_status,
                    statement_timeout_seconds: m.statement_timeout_seconds,
                    storage_name: m.storage_name,
                },
            );
        }

        let mut presets = BTreeMap::new();
        for (name, p) in raw.presets {
            // bounds the interval arithmetic (seconds times 1000, doubled
            // for staleness) inside u64 for any sane schedule
            if p.metrics
                .values()
                .any(|interval| *interval > MAX_INTERVAL_S)
            {
                return Err(err(format!(
                    "preset {name}: interval exceeds {MAX_INTERVAL_S} seconds"
                )));
            }
            presets.insert(
                name.clone(),
                PresetDef {
                    description: p.description,
                    metrics: p.metrics,
                },
            );
        }
        Ok(Self { metrics, presets })
    }

    /// Loads the embedded pgwatch v5.3.0 catalog and verifies its pinned shape.
    ///
    /// # Errors
    ///
    /// Returns a [`CatalogError`] when the embedded YAML fails validation or
    /// its metric/preset counts drift from the pinned values.
    pub fn embedded() -> Result<Self, CatalogError> {
        let catalog = Self::from_yaml_str(EMBEDDED_CATALOG_YAML, "embedded catalog")?;
        if catalog.metrics.len() != EMBEDDED_METRIC_COUNT
            || catalog.presets.len() != EMBEDDED_PRESET_COUNT
        {
            return Err(CatalogError {
                source: "embedded catalog".to_owned(),
                message: format!(
                    "expected {EMBEDDED_METRIC_COUNT} metrics and {EMBEDDED_PRESET_COUNT} presets, got {} and {}",
                    catalog.metrics.len(),
                    catalog.presets.len()
                ),
            });
        }
        Ok(catalog)
    }

    /// Overlays another catalog: definitions replace same-named ones wholly,
    /// unknown names are added (both metrics and presets).
    pub fn overlay(&mut self, overlay: Self) {
        for (name, metric) in overlay.metrics {
            self.metrics.insert(name, metric);
        }
        for (name, preset) in overlay.presets {
            self.presets.insert(name, preset);
        }
    }

    /// Resolves a preset against the catalog, preserving the preset's metric
    /// order for scheduling.
    ///
    /// # Errors
    ///
    /// Returns a [`CatalogError`] when the preset is unknown or names a
    /// metric missing from the catalog.
    pub fn resolve_preset(
        &self,
        preset: &str,
    ) -> Result<Vec<(String, MetricDef, u64)>, CatalogError> {
        let def = self
            .presets
            .get(preset)
            .ok_or_else(|| CatalogError::invalid(format!("preset {preset:?} not found")))?;
        let mut resolved = Vec::with_capacity(def.metrics.len());
        for (name, interval) in &def.metrics {
            let metric = self.metrics.get(name).ok_or_else(|| {
                CatalogError::invalid(format!("preset {preset}: unknown metric {name:?}"))
            })?;
            // Upstream builds its gauges map keyed by the ORIGINAL metric
            // name (DefineMetrics) but looks it up with the storage-resolved
            // name of the running metric (reaper.go MetricName,
            // prometheus.go WritePromMetrics). Net effect, mirrored here: a
            // metric with storage_name=X gets the gauges of the metric
            // literally named X — usually none, so all its columns are
            // counters (e.g. reco_drop_index under storage_name
            // recommendations). This quirk is intentional for parity.
            let mut metric = metric.clone();
            if let Some(storage) = metric.storage_name.as_deref() {
                metric.gauges = self
                    .metrics
                    .get(storage)
                    .map_or(Gauges::Columns(Vec::new()), |owner| owner.gauges.clone());
            }
            resolved.push((name.clone(), metric, *interval));
        }
        Ok(resolved)
    }
}

/// Classifies one SQL variant: runnable single SELECT, or an explicit skip.
///
/// A single trailing `;` is stripped (upstream `archiver_pending_count`
/// ships one). Any other `;` outside strings, dollar quotes and comments is
/// rejected: the executor sends `SET LOCAL …;<newline><sql>` as one
/// simple-protocol message, so an embedded `;` would add statements. A
/// variant consisting only of semicolons and comments is a skip, not an
/// error (upstream `checkpointer` on v14 ships `"; -- covered by bgwriter"`).
fn classify_sql(sql: &str) -> Result<Sql, &'static str> {
    let trimmed = sql.trim_end();
    let stripped = trimmed.strip_suffix(';').unwrap_or(trimmed);
    let chars: Vec<char> = stripped.chars().collect();
    let find_tag = |from: usize, tag: &[char]| -> Option<usize> {
        (from..chars.len().saturating_sub(tag.len() - 1)).find(|&i| chars[i..i + tag.len()] == *tag)
    };
    let mut i = 0;
    let mut first_word = String::new();
    let mut word_done = false;
    let mut statement_text = false; // any non-comment, non-string content
    let mut semicolon = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                // string literal: step over backslash and doubled-quote escapes
                i += 1;
                while i < chars.len() {
                    match chars[i] {
                        '\\' => i += 2,
                        '\'' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                statement_text = true;
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                i += 1;
                statement_text = true;
            }
            '$' if chars
                .get(i + 1)
                .is_some_and(|&n| n == '$' || n.is_ascii_alphanumeric() || n == '_') =>
            {
                // dollar-quoted tag: $tag$ body $tag$ ($$ allowed). A '$'
                // whose tag runs to the end of input (no closing '$') is an
                // ordinary character, not an unterminated quote.
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j < chars.len() && chars[j] == '$' {
                    let tag = &chars[i..=j];
                    i = find_tag(j + 1, tag).map_or(chars.len(), |k| k + tag.len());
                } else {
                    i += 1;
                }
                statement_text = true;
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
            }
            ';' => {
                semicolon = true;
                i += 1;
            }
            c if c.is_whitespace() => {
                if !first_word.is_empty() {
                    word_done = true;
                }
                i += 1;
            }
            c => {
                statement_text = true;
                if !word_done {
                    first_word.push(c.to_ascii_lowercase());
                }
                i += 1;
            }
        }
    }
    if !statement_text {
        return Ok(Sql::Skip);
    }
    if semicolon {
        return Err("embedded semicolon; metric SQL must be a single SELECT");
    }
    if first_word != "select" && first_word != "with" {
        return Err("SQL must start with SELECT or WITH");
    }
    Ok(Sql::Select(stripped.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(yaml: &str) -> Result<Catalog, CatalogError> {
        Catalog::from_yaml_str(yaml, "test.yaml")
    }

    #[test]
    fn embedded_catalog_loads_with_pinned_shape() {
        let catalog = Catalog::embedded().unwrap();
        assert_eq!(catalog.metrics.len(), EMBEDDED_METRIC_COUNT);
        assert_eq!(catalog.presets.len(), EMBEDDED_PRESET_COUNT);
    }

    #[test]
    fn embedded_catalog_loads_through_the_public_loader() {
        // The embedded file must not get a private parser: the same
        // from_yaml_str a user overlay goes through has to accept it.
        let via_loader = Catalog::from_yaml_str(EMBEDDED_CATALOG_YAML, "embedded catalog").unwrap();
        assert_eq!(via_loader, Catalog::embedded().unwrap());
    }

    #[test]
    fn embedded_basic_preset_is_exact() {
        let catalog = Catalog::embedded().unwrap();
        let basic = &catalog.presets["basic"];
        assert_eq!(
            basic.metrics,
            BTreeMap::from([
                ("instance_up".to_owned(), 60),
                ("db_size".to_owned(), 300),
                ("db_stats".to_owned(), 60),
                ("wal".to_owned(), 60),
            ])
        );
    }

    #[test]
    fn embedded_checkpointer_v14_is_skip() {
        let catalog = Catalog::embedded().unwrap();
        let checkpointer = &catalog.metrics["checkpointer"];
        assert!(matches!(checkpointer.sql_for_version(14), Some(Sql::Skip)));
        assert!(matches!(
            checkpointer.sql_for_version(17),
            Some(Sql::Select(_))
        ));
        assert!(matches!(
            checkpointer.sql_for_version(18),
            Some(Sql::Select(_))
        ));
    }

    #[test]
    fn embedded_storage_name_and_gauges() {
        let catalog = Catalog::embedded().unwrap();
        let wal = &catalog.metrics["wal"];
        assert!(wal.is_instance_level);
        assert_eq!(wal.gauges, Gauges::All);
        assert_eq!(wal.exposed_name("wal"), "wal");
        let ssnqt = &catalog.metrics["stat_statements_no_query_text"];
        assert_eq!(
            ssnqt.exposed_name("stat_statements_no_query_text"),
            "stat_statements"
        );
    }

    #[test]
    fn version_selection_picks_newest_not_above_server() {
        let catalog = Catalog::embedded().unwrap();
        let db_stats = &catalog.metrics["db_stats"];
        // real distinguishers: v14 still reads pg_backup_start_time, v15
        // dropped it, v18 adds parallel-worker columns
        let variant = |v: u32| match db_stats.sql_for_version(v).and_then(|s| s.select()) {
            Some(sql) if sql.contains("parallel_workers_launched") => "18",
            Some(sql) if sql.contains("pg_backup_start_time") => "14",
            Some(_) => "15",
            None => "none",
        };
        assert_eq!(variant(14), "14");
        assert_eq!(variant(15), "15");
        assert_eq!(variant(16), "15");
        assert_eq!(variant(17), "15");
        assert_eq!(variant(18), "18");
        assert_eq!(variant(19), "18");
        // a server older than every key gets nothing
        assert_eq!(db_stats.sql_for_version(13), None);
    }

    #[test]
    fn version_selection_exact_and_closest_below() {
        let def = MetricDef {
            description: String::new(),
            sqls: BTreeMap::from([
                (14, Sql::Select("a".to_owned())),
                (17, Sql::Select("b".to_owned())),
            ]),
            gauges: Gauges::Columns(vec![]),
            is_instance_level: false,
            node_status: None,
            statement_timeout_seconds: None,
            storage_name: None,
        };
        assert_eq!(def.sql_for_version(14).unwrap().select(), Some("a"));
        assert_eq!(def.sql_for_version(16).unwrap().select(), Some("a"));
        assert_eq!(def.sql_for_version(17).unwrap().select(), Some("b"));
        assert_eq!(def.sql_for_version(20).unwrap().select(), Some("b"));
        assert_eq!(def.sql_for_version(13), None);
    }

    #[test]
    fn overlay_replaces_whole_metric_and_adds_new() {
        let mut base = load("metrics:\n  m:\n    sqls:\n      14: 'select 1 as a'\n").unwrap();
        let overlay = load(
            "metrics:\n  m:\n    description: replaced\n    sqls:\n      16: 'select 2 as b'\n  n:\n    sqls:\n      14: 'select 3 as c'\n",
        )
        .unwrap();
        base.overlay(overlay);
        let m = &base.metrics["m"];
        assert_eq!(m.description, "replaced");
        // replacement is whole: the v14 variant from the base is gone
        assert_eq!(m.sqls.keys().copied().collect::<Vec<_>>(), [16]);
        assert!(base.metrics.contains_key("n"));
    }

    #[test]
    fn overlay_replaces_and_adds_presets() {
        let mut base = load("presets:\n  p:\n    metrics: {m: 60}\n").unwrap();
        let overlay =
            load("presets:\n  p:\n    metrics: {m: 30}\n  q:\n    metrics: {m: 10}\n").unwrap();
        base.overlay(overlay);
        assert_eq!(base.presets["p"].metrics["m"], 30);
        assert_eq!(base.presets["q"].metrics["m"], 10);
    }

    #[test]
    fn unknown_key_is_rejected_with_line() {
        let err = load("metrics:\n  m:\n    sqls: {14: 'select 1'}\n    bogus: 1\n").unwrap_err();
        assert!(err.message.contains("line 4"), "{err}");
        assert_eq!(err.source, "test.yaml");
    }

    #[test]
    fn init_sql_and_is_private_are_accepted_and_ignored() {
        let catalog = load(
            "metrics:\n  m:\n    sqls: {14: 'select 1 as a'}\n    init_sql: 'create extension x'\n    is_private: true\n",
        )
        .unwrap();
        assert_eq!(catalog.metrics.len(), 1);
    }

    #[test]
    fn sql_forms() {
        let skip = |sql: &str| classify_sql(sql).unwrap() == Sql::Skip;
        assert!(skip(""));
        assert!(skip("   \n -- nothing\n"));
        assert!(skip("; -- covered by bgwriter"));
        assert!(classify_sql("select 1").unwrap() == Sql::Select("select 1".to_owned()));
        assert!(classify_sql("  with x as (select 1) select * from x").is_ok());
        // semicolons inside string literals and comments are fine
        assert!(classify_sql("select ';' as x").is_ok());
        assert!(classify_sql("select 1 -- ; trailing comment").is_ok());
        assert!(classify_sql("select 1 /* ; */ as x").is_ok());
        // a single trailing semicolon is stripped: upstream
        // archiver_pending_count ships one
        assert!(classify_sql("select 1;").is_ok());
        assert!(classify_sql("select 1 ;\n").is_ok());
        assert_eq!(
            classify_sql("select 'x';").unwrap(),
            Sql::Select("select 'x'".to_owned())
        );
        // an embedded semicolon splits statements and is rejected
        assert!(classify_sql("select 1; select 2").is_err());
        assert!(classify_sql("select 1; -- done").is_err());
        assert!(classify_sql("update t set x = 1").is_err());
        assert!(classify_sql("explain select 1").is_err());
        // dollar-quoted bodies may contain anything
        assert!(classify_sql("select foo($$ x ; y $$) as v").is_ok());
        // a '$' whose tag runs to the end of input is ordinary text, not a
        // panic and not an unterminated quote
        assert!(classify_sql("select 1 where a = b$c").is_ok());
        assert!(classify_sql("select $abc").is_ok());
        assert!(classify_sql("select foo($tag$ a ; b $tag$) as v").is_ok());
    }

    #[test]
    fn node_status_and_timeout_validation() {
        // unknown values load as unrestricted (upstream free-string parse)
        let ok =
            load("metrics:\n  m:\n    sqls: {14: 'select 1'}\n    node_status: replica\n").unwrap();
        assert_eq!(ok.metrics["m"].node_status, None);
        let ok =
            load("metrics:\n  m:\n    sqls: {14: 'select 1'}\n    node_status: standby\n").unwrap();
        assert_eq!(ok.metrics["m"].node_status, Some(NodeStatus::Standby));
        let err =
            load("metrics:\n  m:\n    sqls: {14: 'select 1'}\n    statement_timeout_seconds: 0\n")
                .unwrap_err();
        assert!(err.message.contains("statement_timeout_seconds"), "{err}");
    }

    #[test]
    fn gauges_forms() {
        let catalog = load("metrics:\n  m:\n    sqls: {14: 'select 1'}\n    gauges: ['*']\n  n:\n    sqls: {14: 'select 1'}\n    gauges: [a, b]\n").unwrap();
        assert_eq!(catalog.metrics["m"].gauges, Gauges::All);
        assert_eq!(
            catalog.metrics["n"].gauges,
            Gauges::Columns(vec!["a".to_owned(), "b".to_owned()])
        );
        assert!(catalog.metrics["n"].gauges.contains("b"));
        assert!(!catalog.metrics["n"].gauges.contains("c"));
        // a '*' after position 0 is an ordinary entry: it stays in the list
        // and only ever matches a column literally named that way
        let ok =
            load("metrics:\n  m:\n    sqls: {14: 'select 1'}\n    gauges: [a, '*']\n").unwrap();
        assert_eq!(
            ok.metrics["m"].gauges,
            Gauges::Columns(vec!["a".to_owned(), "*".to_owned()])
        );
        assert!(ok.metrics["m"].gauges.contains("a"));
        assert!(!ok.metrics["m"].gauges.contains("b"));
    }

    #[test]
    fn storage_name_resolves_gauges_by_exposed_name() {
        // upstream looks gauges up by the storage-resolved name: a metric
        // with storage_name X gets the gauges of the metric named X
        let catalog = load(
            "metrics:\n  a:\n    sqls: {14: 'select 1 as x'}\n    gauges: [x]\n    storage_name: b\n  b:\n    sqls: {14: 'select 1 as x'}\n  c:\n    sqls: {14: 'select 1 as y'}\n    storage_name: d\n  d:\n    sqls: {14: 'select 1 as y'}\n    gauges: [y]\npresets:\n  p:\n    metrics: {a: 60, c: 60, b: 60, d: 60}\n",
        )
        .unwrap();
        let resolved = catalog.resolve_preset("p").unwrap();
        let gauges_of = |name: &str| {
            resolved
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, def, _)| def.gauges.clone())
                .unwrap()
        };
        // a exposes as b, whose own gauges are empty: all counters
        assert_eq!(gauges_of("a"), Gauges::Columns(Vec::new()));
        // c exposes as d and inherits d's gauge list
        assert_eq!(gauges_of("c"), Gauges::Columns(vec!["y".to_owned()]));
        // metrics without storage_name keep their own list
        assert_eq!(gauges_of("d"), Gauges::Columns(vec!["y".to_owned()]));
    }

    #[test]
    fn preset_resolution_reports_unknown_names() {
        let mut catalog = Catalog::default();
        catalog.presets.insert(
            "p".to_owned(),
            PresetDef {
                description: String::new(),
                metrics: BTreeMap::from([("m".to_owned(), 60)]),
            },
        );
        let err = catalog.resolve_preset("p").unwrap_err();
        assert!(err.message.contains("unknown metric \"m\""), "{err}");
        catalog.metrics.insert(
            "m".to_owned(),
            MetricDef {
                description: String::new(),
                sqls: BTreeMap::from([(14, Sql::Select("select 1".to_owned()))]),
                gauges: Gauges::Columns(vec![]),
                is_instance_level: false,
                node_status: None,
                statement_timeout_seconds: None,
                storage_name: None,
            },
        );
        let resolved = catalog.resolve_preset("p").unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].2, 60);
        assert!(catalog.resolve_preset("missing").is_err());
    }
}
