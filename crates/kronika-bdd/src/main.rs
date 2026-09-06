//! Runs BDD scenarios against shipped binaries and artifacts.
#![allow(
    clippy::trivial_regex,
    reason = "cucumber step phrases are literal English, matched as plain text, not real regexes"
)]
#![allow(
    clippy::needless_pass_by_ref_mut,
    reason = "cucumber passes &mut World to every step by contract, even read-only ones"
)]
#![allow(
    clippy::needless_pass_by_value,
    reason = "cucumber builds a step's captured parameters by value; a borrow does not bind"
)]

mod collector;
mod demo;
mod services;
mod steps;

use collector::Run;
use cucumber::World as _;

#[derive(Debug, Default, cucumber::World)]
struct BddWorld {
    env: Vec<(String, String)>,
    demo_env: Vec<(String, String)>,
    fixture: Option<tempfile::TempDir>,
    prepared_root: Option<tempfile::TempDir>,
    run: Option<Run>,
    demo: Option<demo::DemoRun>,
    postgres: Option<services::Postgres>,
    pgbouncer: Option<services::PgBouncer>,
}

#[tokio::main]
async fn main() {
    let features = std::env::var("KRONIKA_FEATURES").unwrap_or_else(|_unset| "features".to_owned());
    // Services use fixed ports and data directories, so scenarios run serially.
    BddWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(features)
        .await;
}
