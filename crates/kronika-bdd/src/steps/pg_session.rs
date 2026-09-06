use std::collections::BTreeMap;
use std::convert::Infallible;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
use kronika_source_pg::query::{self, BatchError, BatchWrite, QueryStats};
use kronika_source_pg::{Pool, databases, user_indexes};
use tokio_postgres::NoTls;

use super::table_rows;
use crate::BddWorld;

#[then("the monitoring index read rejects the lock wait and recovers on the same session")]
async fn index_lock_wait(world: &mut BddWorld, step: &Step) -> Result<()> {
    let postgres = world
        .postgres
        .as_ref()
        .context("a PostgreSQL was started")?;
    let mut settings = BTreeMap::new();
    for row in table_rows(step, &["setting", "value"])? {
        let [key, value] = row.as_slice() else {
            anyhow::bail!("a session setting needs a key and value, got {row:?}");
        };
        anyhow::ensure!(
            settings.insert(key.clone(), value.clone()).is_none(),
            "duplicate session setting {key}"
        );
    }
    let get = |key| {
        settings
            .get(key)
            .map(String::as_str)
            .with_context(|| format!("missing session setting {key}"))
    };
    let maximum_wait = Duration::from_millis(get("maximum wait ms")?.parse()?);
    let (blocker, connection) = tokio_postgres::connect(&postgres.dsn, NoTls).await?;
    let driver = tokio::spawn(connection);
    let result = async {
        blocker.batch_execute(get("setup SQL")?).await?;
        let mut pool = Pool::new(&postgres.dsn)?;
        let session = pool.session().await?;
        let mut stats = QueryStats::default();
        assert_timeouts(
            session,
            &mut stats,
            get("statement timeout")?,
            get("lock timeout")?,
        )
        .await?;
        let database = databases::enumerate(session, &mut stats)
            .await?
            .into_iter()
            .find(|database| database.is_current)
            .context("the connected database was enumerated")?;
        let version =
            query::read_simple_i32(session, "SHOW server_version_num", &mut stats).await?;
        let major = u32::try_from(version)? / 10_000;
        let pid = query::read_simple_i32(session, "SELECT pg_backend_pid()", &mut stats).await?;

        // pg_get_indexdef opens the indexed table with AccessShareLock.
        blocker.batch_execute(get("lock SQL")?).await?;
        let started = Instant::now();
        let collected = tokio::time::timeout(
            maximum_wait,
            user_indexes::collect_user_indexes(session, &database, major, &mut stats, |_batch| {
                Ok::<_, Infallible>(BatchWrite::default())
            }),
        )
        .await
        .context("the index read exceeded the scenario's lock-wait bound")?;
        let elapsed = started.elapsed();
        let Err(BatchError::PostgreSql(error)) = collected else {
            anyhow::bail!("expected a PostgreSQL lock error, got {collected:?}");
        };
        anyhow::ensure!(
            error.code().map(tokio_postgres::error::SqlState::code) == Some(get("SQLSTATE")?),
            "unexpected index read error: {error:?}"
        );
        anyhow::ensure!(elapsed < maximum_wait, "the lock wait took {elapsed:?}");

        let unrelated = query::read_simple_i32(session, get("unrelated SQL")?, &mut stats).await?;
        anyhow::ensure!(
            unrelated == get("unrelated result")?.parse::<i32>()?,
            "the unrelated read returned {unrelated}"
        );
        let same_pid =
            query::read_simple_i32(session, "SELECT pg_backend_pid()", &mut stats).await?;
        anyhow::ensure!(same_pid == pid, "the monitoring session was replaced");
        assert_idle_backend(
            &blocker,
            pid,
            get("backend state")?,
            get("waiting locks")?.parse()?,
            get("open transactions")?.parse()?,
        )
        .await?;

        blocker.batch_execute(get("release SQL")?).await?;
        let mut found = false;
        let expected_index = get("expected index")?;
        user_indexes::collect_user_indexes(session, &database, major, &mut stats, |batch| {
            found |= batch
                .rows
                .iter()
                .any(|row| row.indexrelname == expected_index);
            Ok::<_, Infallible>(BatchWrite::default())
        })
        .await
        .map_err(|error| anyhow::anyhow!("index read after lock release failed: {error:?}"))?;
        anyhow::ensure!(found, "the retry did not return index {expected_index}");
        Ok(())
    }
    .await;
    // Closing the fixture connection also releases its lock on an assertion failure.
    driver.abort();
    result
}

async fn assert_idle_backend(
    observer: &tokio_postgres::Client,
    pid: i32,
    expected_state: &str,
    expected_waiting_locks: i64,
    expected_open_transactions: i32,
) -> Result<()> {
    let backend = observer
        .query_one(
            "SELECT state, (xact_start IS NOT NULL)::integer AS open_transactions, \
             (SELECT count(*) FROM pg_catalog.pg_locks WHERE pid = $1 AND NOT granted) \
                AS waiting_locks \
             FROM pg_catalog.pg_stat_activity WHERE pid = $1",
            &[&pid],
        )
        .await?;
    let state: &str = backend.try_get("state")?;
    let waiting_locks: i64 = backend.try_get("waiting_locks")?;
    let open_transactions: i32 = backend.try_get("open_transactions")?;
    anyhow::ensure!(state == expected_state, "backend state is {state}");
    anyhow::ensure!(
        waiting_locks == expected_waiting_locks,
        "backend has {waiting_locks} waiting locks"
    );
    anyhow::ensure!(
        open_transactions == expected_open_transactions,
        "backend has {open_transactions} open transactions"
    );
    Ok(())
}

async fn assert_timeouts(
    session: query::Session<'_>,
    stats: &mut QueryStats,
    statement_timeout: &str,
    lock_timeout: &str,
) -> Result<()> {
    for (sql, expected) in [
        ("SHOW statement_timeout", statement_timeout),
        ("SHOW lock_timeout", lock_timeout),
    ] {
        let rows = query::read_simple_rows(session, sql, stats, |row| {
            row.get(0)
                .context("SHOW returned no value")
                .map(str::to_owned)
        })
        .await?;
        anyhow::ensure!(
            rows == [expected],
            "{sql} returned {rows:?}, expected {expected}"
        );
    }
    Ok(())
}
