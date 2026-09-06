use super::table_rows;
use crate::BddWorld;
use crate::collector::{Run, files_under};
use crate::demo::DemoRun;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::when;
use std::path::PathBuf;
use std::time::Duration;

#[when(regex = r"^it runs for (\d+) seconds$")]
fn run_for(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let run = world.run.as_mut().context("a collector was started")?;
    run.run_for_and_stop(Duration::from_secs(seconds))
}

#[when(regex = r"^the demo finishes within (\d+) seconds$")]
fn demo_finishes(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let mut demo = DemoRun::spawn(&world.demo_env)?;
    demo.wait(Duration::from_secs(seconds))?;
    world.demo = Some(demo);
    Ok(())
}

#[when(regex = r"^the last (\d+) bytes of the journal are cut off$")]
fn cut_the_journal(world: &mut BddWorld, bytes: u64) -> Result<()> {
    let run = world.run.take().context("a collector was started")?;
    let journal = run.storage_dir().join("active.wal");
    let len = std::fs::metadata(&journal)
        .with_context(|| format!("stat {}", journal.display()))?
        .len();
    anyhow::ensure!(
        len > bytes,
        "the journal holds {len} bytes, too few to cut {bytes}"
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&journal)?
        .set_len(len - bytes)?;
    world.prepared_root = Some(run.into_root());
    Ok(())
}

#[when(regex = r"^the oldest published segment is cut down to (\d+) bytes$")]
fn cut_a_segment(world: &mut BddWorld, bytes: u64) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let root = run.storage_dir();
    let mut segments: Vec<PathBuf> = files_under(&root)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "zms"))
        .collect();
    segments.sort();
    let oldest = root.join(segments.first().context("no segment was published")?);
    let len = std::fs::metadata(&oldest)
        .with_context(|| format!("stat {}", oldest.display()))?
        .len();
    anyhow::ensure!(
        len > bytes,
        "{} holds {len} bytes, too few to cut down to {bytes}",
        oldest.display()
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&oldest)?
        .set_len(bytes)?;
    Ok(())
}

#[when(regex = r"^it starts again over the same data root and runs for (\d+) seconds$")]
fn restart_over_the_same_root(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let root = world
        .prepared_root
        .take()
        .context("a data root was prepared")?;
    let mut run = Run::spawn(root, &world.env)?;
    run.run_for_and_stop(Duration::from_secs(seconds))?;
    world.run = Some(run);
    Ok(())
}

#[when("these statements run against PostgreSQL")]
fn statements_run(world: &mut BddWorld, step: &Step) -> Result<()> {
    let postgres = world
        .postgres
        .as_ref()
        .context("a PostgreSQL was started")?;
    for row in table_rows(step, &["statement"])? {
        let [statement] = row.as_slice() else {
            anyhow::bail!("a statement row holds one statement, got {row:?}");
        };
        postgres.statement(statement)?;
    }
    Ok(())
}

#[when("these clients connect through PgBouncer")]
fn clients_connect(world: &mut BddWorld, step: &Step) -> Result<()> {
    let pgbouncer = world
        .pgbouncer
        .as_ref()
        .context("a PgBouncer was started")?;
    for row in table_rows(step, &["database"])? {
        let [database] = row.as_slice() else {
            anyhow::bail!("a client row holds one database, got {row:?}");
        };
        pgbouncer.connect_to(database)?;
    }
    Ok(())
}
