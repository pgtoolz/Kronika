//! Run a collector demonstration with bounded activity and a measurement report.

mod collector;
mod config;
mod report;
mod run;
mod sample;
mod sections;
mod shutdown;
mod system_activity;
mod workload;

fn main() -> anyhow::Result<()> {
    let config = config::parse_from(std::env::args_os()).unwrap_or_else(|error| error.exit());
    run::run(config)
}

#[cfg(test)]
#[path = "tests/packaging.rs"]
mod packaging_tests;
