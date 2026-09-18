//! `PostgreSQL` workload settings and hard bounds, validated before startup.

use super::{locks, naming};
use crate::config::{integer, milliseconds, option, seconds, text};
use anyhow::{Context, Result};
use std::fmt;

#[derive(Clone)]
pub(crate) struct WorkloadConfig {
    /// DML connection, normally through `PgBouncer`.
    pub(crate) dsn: String,
    /// Direct connection for session-scoped settings.
    pub(crate) direct_dsn: String,
    pub(crate) schemas: u32,
    pub(crate) tables_per_schema: u32,
    pub(crate) ddl_concurrency: u32,
    pub(crate) sessions: u32,
    pub(crate) transactions_per_second: u32,
    pub(crate) max_orders: u32,
    pub(crate) lock_chains: u32,
    pub(crate) lock_chain_depth: u32,
    pub(crate) lock_hold_ms: u64,
    pub(crate) lock_round_interval_s: u64,
    pub(crate) event_round_interval_s: u64,
    pub(crate) plan_rows: u32,
    pub(crate) plan_workers: u32,
    pub(crate) plan_baseline_s: u64,
    pub(crate) plan_regression_s: u64,
    pub(crate) plan_round_interval_s: u64,
    pub(crate) vacuum_rows: u32,
    pub(crate) vacuum_round_interval_s: u64,
    pub(crate) vacuum_statement_timeout_s: u64,
}

impl fmt::Debug for WorkloadConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkloadConfig")
            .field("dsn", &"[redacted]")
            .field("direct_dsn", &"[redacted]")
            .field("schemas", &self.schemas)
            .field("tables_per_schema", &self.tables_per_schema)
            .field("ddl_concurrency", &self.ddl_concurrency)
            .field("sessions", &self.sessions)
            .field("transactions_per_second", &self.transactions_per_second)
            .field("max_orders", &self.max_orders)
            .field("lock_chains", &self.lock_chains)
            .field("lock_chain_depth", &self.lock_chain_depth)
            .field("lock_hold_ms", &self.lock_hold_ms)
            .field("lock_round_interval_s", &self.lock_round_interval_s)
            .field("event_round_interval_s", &self.event_round_interval_s)
            .field("plan_rows", &self.plan_rows)
            .field("plan_workers", &self.plan_workers)
            .field("plan_baseline_s", &self.plan_baseline_s)
            .field("plan_regression_s", &self.plan_regression_s)
            .field("plan_round_interval_s", &self.plan_round_interval_s)
            .field("vacuum_rows", &self.vacuum_rows)
            .field("vacuum_round_interval_s", &self.vacuum_round_interval_s)
            .field(
                "vacuum_statement_timeout_s",
                &self.vacuum_statement_timeout_s,
            )
            .finish()
    }
}

pub(super) fn required_direct_dsn(raw: Option<String>) -> Result<String> {
    let dsn = raw.context(
        "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN must be set to a direct PostgreSQL connection when KRONIKA_DEMO_WORKLOAD_DSN enables the workload",
    )?;
    anyhow::ensure!(
        !dsn.trim().is_empty(),
        "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN must not be blank"
    );
    Ok(dsn)
}

impl WorkloadConfig {
    pub(crate) fn from_matches(matches: &clap::ArgMatches) -> Result<Option<Self>> {
        if !matches.contains_id("KRONIKA_DEMO_WORKLOAD_DSN") {
            return Ok(None);
        }
        let dsn = text(matches, "KRONIKA_DEMO_WORKLOAD_DSN")?.to_owned();
        let direct_dsn = required_direct_dsn(
            matches
                .contains_id("KRONIKA_DEMO_WORKLOAD_DIRECT_DSN")
                .then(|| text(matches, "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN"))
                .transpose()?
                .map(str::to_owned),
        )?;
        let config = Self {
            dsn,
            direct_dsn,
            schemas: integer(matches, "KRONIKA_DEMO_WORKLOAD_SCHEMAS")?,
            tables_per_schema: integer(matches, "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA")?,
            ddl_concurrency: integer(matches, "KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY")?,
            sessions: integer(matches, "KRONIKA_DEMO_WORKLOAD_SESSIONS")?,
            transactions_per_second: integer(matches, "KRONIKA_DEMO_WORKLOAD_TPS")?,
            max_orders: integer(matches, "KRONIKA_DEMO_WORKLOAD_MAX_ORDERS")?,
            lock_chains: integer(matches, "KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS")?,
            lock_chain_depth: integer(matches, "KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH")?,
            lock_hold_ms: milliseconds(matches, "KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS")?,
            lock_round_interval_s: seconds(matches, "KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S")?,
            event_round_interval_s: seconds(
                matches,
                "KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S",
            )?,
            plan_rows: integer(matches, "KRONIKA_DEMO_WORKLOAD_PLAN_ROWS")?,
            plan_workers: integer(matches, "KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS")?,
            plan_baseline_s: seconds(matches, "KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S")?,
            plan_regression_s: seconds(matches, "KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S")?,
            plan_round_interval_s: seconds(matches, "KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S")?,
            vacuum_rows: integer(matches, "KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS")?,
            vacuum_round_interval_s: seconds(
                matches,
                "KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S",
            )?,
            vacuum_statement_timeout_s: seconds(
                matches,
                "KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S",
            )?,
        };
        config.validate()?;
        Ok(Some(config))
    }

    pub(super) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.dsn.trim().is_empty(),
            "KRONIKA_DEMO_WORKLOAD_DSN must not be blank"
        );
        for (key, dsn) in [
            ("KRONIKA_DEMO_WORKLOAD_DSN", &self.dsn),
            ("KRONIKA_DEMO_WORKLOAD_DIRECT_DSN", &self.direct_dsn),
        ] {
            dsn.parse::<tokio_postgres::Config>().map_err(|_error| {
                anyhow::anyhow!("{key} is not a valid PostgreSQL connection string")
            })?;
        }
        self.validate_dimensions()?;
        self.validate_timings()
    }

    fn validate_dimensions(&self) -> Result<()> {
        for (key, value, minimum, maximum) in [
            ("KRONIKA_DEMO_WORKLOAD_SCHEMAS", self.schemas, 1, 8),
            (
                "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA",
                self.tables_per_schema,
                naming::COMMERCE_TABLE_COUNT,
                64,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY",
                self.ddl_concurrency,
                1,
                16,
            ),
            ("KRONIKA_DEMO_WORKLOAD_SESSIONS", self.sessions, 1, 16),
            (
                "KRONIKA_DEMO_WORKLOAD_TPS",
                self.transactions_per_second,
                1,
                64,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_MAX_ORDERS",
                self.max_orders,
                1,
                50_000,
            ),
            ("KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS", self.lock_chains, 1, 4),
            (
                "KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH",
                self.lock_chain_depth,
                2,
                8,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_PLAN_ROWS",
                self.plan_rows,
                1,
                500_000,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS",
                self.plan_workers,
                1,
                8,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS",
                self.vacuum_rows,
                1,
                250_000,
            ),
        ] {
            anyhow::ensure!(
                (minimum..=maximum).contains(&value),
                "{key} must be between {minimum} and {maximum}"
            );
        }
        anyhow::ensure!(
            self.max_orders >= self.sessions,
            "KRONIKA_DEMO_WORKLOAD_MAX_ORDERS must be at least KRONIKA_DEMO_WORKLOAD_SESSIONS"
        );
        Ok(())
    }

    fn validate_timings(&self) -> Result<()> {
        anyhow::ensure!(
            locks::round_has_timed_out_tail(self.lock_chain_depth, self.lock_hold_ms),
            "lock timing must let an earlier waiter acquire the row and a later waiter reach statement_timeout"
        );
        for (key, value) in [
            ("KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS", self.lock_hold_ms),
            (
                "KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S",
                self.lock_round_interval_s,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S",
                self.event_round_interval_s,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S",
                self.plan_baseline_s,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S",
                self.plan_regression_s,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S",
                self.plan_round_interval_s,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S",
                self.vacuum_round_interval_s,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S",
                self.vacuum_statement_timeout_s,
            ),
        ] {
            anyhow::ensure!(value > 0, "{key} must be greater than zero");
        }
        Ok(())
    }
}

const OPTIONS: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "workload-dsn",
        "KRONIKA_DEMO_WORKLOAD_DSN",
        None,
        "Enable PostgreSQL OLTP activity using this connection string",
    ),
    (
        "workload-direct-dsn",
        "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN",
        None,
        "Direct PostgreSQL connection for session settings; required with workload-dsn",
    ),
    (
        "workload-schemas",
        "KRONIKA_DEMO_WORKLOAD_SCHEMAS",
        Some("1"),
        "Commerce schemas (1..8)",
    ),
    (
        "workload-tables-per-schema",
        "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA",
        Some("8"),
        "Tables per schema (8..64)",
    ),
    (
        "workload-ddl-concurrency",
        "KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY",
        Some("4"),
        "Concurrent setup connections (1..16)",
    ),
    (
        "workload-sessions",
        "KRONIKA_DEMO_WORKLOAD_SESSIONS",
        Some("4"),
        "Persistent OLTP clients (1..16)",
    ),
    (
        "workload-tps",
        "KRONIKA_DEMO_WORKLOAD_TPS",
        Some("20"),
        "Maximum aggregate OLTP transactions per second (1..64)",
    ),
    (
        "workload-max-orders",
        "KRONIKA_DEMO_WORKLOAD_MAX_ORDERS",
        Some("10000"),
        "Reusable order IDs (session count..50000)",
    ),
    (
        "workload-lock-chains",
        "KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS",
        Some("1"),
        "Independent lock chains per round (1..4)",
    ),
    (
        "workload-lock-chain-depth",
        "KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH",
        Some("4"),
        "Transactions per chain (2..8); timing must produce a timed-out tail",
    ),
    (
        "workload-lock-hold-ms",
        "KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS",
        Some("4000"),
        "Row-lock hold time per link in milliseconds",
    ),
    (
        "workload-lock-round-interval-s",
        "KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S",
        Some("120"),
        "Quiet interval after each lock round in seconds (> 0)",
    ),
    (
        "workload-event-round-interval-s",
        "KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S",
        Some("180"),
        "Interval after each slow-query/error episode in seconds (> 0)",
    ),
    (
        "workload-plan-rows",
        "KRONIKA_DEMO_WORKLOAD_PLAN_ROWS",
        Some("300000"),
        "Rows for the plan-regression scenario (1..500000)",
    ),
    (
        "workload-plan-workers",
        "KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS",
        Some("4"),
        "Concurrent checkout-api plan sessions (1..8)",
    ),
    (
        "workload-plan-baseline-s",
        "KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S",
        Some("12"),
        "Indexed plan baseline duration in seconds (> 0)",
    ),
    (
        "workload-plan-regression-s",
        "KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S",
        Some("30"),
        "Unindexed plan duration in seconds (> 0)",
    ),
    (
        "workload-plan-round-interval-s",
        "KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S",
        Some("120"),
        "Interval after each plan round in seconds (> 0)",
    ),
    (
        "workload-vacuum-rows",
        "KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS",
        Some("100000"),
        "Rows in the vacuum scenario (1..250000)",
    ),
    (
        "workload-vacuum-round-interval-s",
        "KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S",
        Some("180"),
        "Interval after each vacuum episode in seconds (> 0)",
    ),
    (
        "workload-vacuum-statement-timeout-s",
        "KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S",
        Some("30"),
        "Timeout per vacuum/update statement in seconds (> 0)",
    ),
];

pub(crate) fn args() -> impl Iterator<Item = clap::Arg> {
    OPTIONS.iter().map(|&(long, key, default, help)| {
        let arg = option(long, key, default, help).help_heading("PostgreSQL workload");
        if key.ends_with("_DSN") {
            arg.value_name("DSN")
        } else {
            arg
        }
    })
}
