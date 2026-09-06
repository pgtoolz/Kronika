use super::table_rows;
use crate::BddWorld;
use crate::collector::{Run, copy_tree};
use crate::services::{PgBouncer, Postgres};
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::given;
use kronika_format::JOURNAL_MAGIC;
use std::path::PathBuf;

const FIXTURE_PATH: &str = "{fixture}";

fn fixtures_dir() -> PathBuf {
    std::env::var("KRONIKA_FIXTURES").map_or_else(|_unset| PathBuf::from("fixtures"), PathBuf::from)
}

#[given("a collector with these settings")]
fn collector_with_settings(world: &mut BddWorld, step: &Step) -> Result<()> {
    for row in table_rows(step, &["variable", "value"])? {
        let [key, value] = row.as_slice() else {
            anyhow::bail!("a settings row needs a variable and a value, got {row:?}");
        };
        let value = if value.contains(FIXTURE_PATH) {
            let fixture = world
                .fixture
                .as_ref()
                .context("a filesystem fixture exists")?;
            value.replace(FIXTURE_PATH, &fixture.path().to_string_lossy())
        } else {
            value.clone()
        };
        world.env.push((key.clone(), value));
    }
    let root = world
        .prepared_root
        .take()
        .map_or_else(|| tempfile::tempdir().context("create a data root"), Ok)?;
    world.run = Some(Run::spawn(root, &world.env)?);
    Ok(())
}

#[given("a demo workload with these settings")]
fn demo_with_settings(world: &mut BddWorld, step: &Step) -> Result<()> {
    for row in table_rows(step, &["variable", "value"])? {
        let [key, value] = row.as_slice() else {
            anyhow::bail!("a settings row needs a variable and a value, got {row:?}");
        };
        world.demo_env.push((key.clone(), value.clone()));
    }
    Ok(())
}

#[given(regex = r"^a procfs fixture named (\S+)$")]
fn procfs_fixture(world: &mut BddWorld, name: String) -> Result<()> {
    let source = fixtures_dir().join(&name);
    anyhow::ensure!(
        source.is_dir(),
        "no fixture named {name} under {}",
        fixtures_dir().display()
    );
    let fixture = tempfile::tempdir().context("create a fixture root")?;
    let root = fixture.path().join(&name);
    copy_tree(&source, &root)?;
    world.env.push((
        "KRONIKA_PROC_ROOT".to_owned(),
        root.to_string_lossy().into_owned(),
    ));
    world.fixture = Some(fixture);
    Ok(())
}

#[given(regex = r#"^a data root whose active\.wal is the journal magic followed by "(.+)"$"#)]
fn prepared_corrupt_journal(world: &mut BddWorld, trailer: String) -> Result<()> {
    let root = tempfile::tempdir().context("create a data root")?;
    let storage_dir = root.path().join("segments");
    std::fs::create_dir_all(&storage_dir).context("create the segments directory")?;
    let mut bytes = JOURNAL_MAGIC.to_vec();
    bytes.extend_from_slice(trailer.as_bytes());
    std::fs::write(storage_dir.join("active.wal"), bytes).context("write the damaged journal")?;
    world.prepared_root = Some(root);
    Ok(())
}

#[given(regex = r"^a PostgreSQL writing its log as (\S+)$")]
fn postgres_writing_its_log(world: &mut BddWorld, destination: String) -> Result<()> {
    world.postgres = Some(Postgres::start(&destination)?);
    Ok(())
}

#[given("a PgBouncer in front of it")]
fn pgbouncer_in_front(world: &mut BddWorld) -> Result<()> {
    world.pgbouncer = Some(PgBouncer::start()?);
    Ok(())
}

#[given("the collector reaches PostgreSQL by DSN")]
fn postgres_by_dsn(world: &mut BddWorld) {
    let dsn = world.postgres.as_ref().expect("a server").dsn.clone();
    world.env.push(("KRONIKA_PG_DSNS".to_owned(), dsn));
}

#[given("the collector is told the PostgreSQL log path")]
fn postgres_by_path(world: &mut BddWorld) {
    let path = world.postgres.as_ref().expect("a server").log_path.clone();
    world.env.push((
        "KRONIKA_PG_LOGS".to_owned(),
        path.to_string_lossy().into_owned(),
    ));
}

#[given("the demo workload uses that PostgreSQL")]
fn demo_uses_postgres(world: &mut BddWorld) -> Result<()> {
    let dsn = world
        .postgres
        .as_ref()
        .context("a PostgreSQL was started")?
        .dsn
        .clone();
    world
        .demo_env
        .push(("KRONIKA_DEMO_WORKLOAD_DSN".to_owned(), dsn.clone()));
    world
        .demo_env
        .push(("KRONIKA_DEMO_WORKLOAD_DIRECT_DSN".to_owned(), dsn));
    Ok(())
}

#[given("the demo workload reaches PostgreSQL through PgBouncer")]
fn demo_uses_pgbouncer(world: &mut BddWorld) -> Result<()> {
    let workload_dsn = world
        .pgbouncer
        .as_ref()
        .context("a PgBouncer was started")?
        .workload_dsn
        .clone();
    let direct_dsn = world
        .postgres
        .as_ref()
        .context("a PostgreSQL was started")?
        .dsn
        .clone();
    world
        .demo_env
        .push(("KRONIKA_DEMO_WORKLOAD_DSN".to_owned(), workload_dsn));
    world
        .demo_env
        .push(("KRONIKA_DEMO_WORKLOAD_DIRECT_DSN".to_owned(), direct_dsn));
    Ok(())
}

#[given("the collector reaches PgBouncer by DSN")]
fn pgbouncer_by_dsn(world: &mut BddWorld) {
    let dsn = world.pgbouncer.as_ref().expect("a pooler").dsn.clone();
    world.env.push(("KRONIKA_PGBOUNCER_DSNS".to_owned(), dsn));
}

#[given(regex = "^the collector is told the PgBouncer logs as (.+)$")]
fn pgbouncer_by_path(world: &mut BddWorld, entry: String) {
    world.env.push(("KRONIKA_PGBOUNCER_LOGS".to_owned(), entry));
}
