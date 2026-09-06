use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, Ordering};

const PG_DATA: &str = "/tmp/kronika-pgdata";
const PGB_DIR: &str = "/tmp/kronika-pgbouncer";
const PG_USER: &str = "postgres";
const PG_PORT: &str = "5432";
// Process-derived ports separate parallel suite processes.
fn next_pgb_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    static BASE: OnceLock<u16> = OnceLock::new();
    let base =
        *BASE.get_or_init(|| 20_000 + u16::try_from(std::process::id() % 20_000).unwrap_or(0));
    base + NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
pub(crate) struct Postgres {
    pub(crate) log_path: PathBuf,
    pub(crate) dsn: String,
    psql: String,
}

#[derive(Debug)]
pub(crate) struct PgBouncer {
    pub(crate) dsn: String,
    pub(crate) workload_dsn: String,
    psql: String,
}

pub(crate) fn run(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("run {}", command.get_program().display()))?;
    if !output.status.success() {
        bail!(
            "{} {:?} failed: {}{}",
            command.get_program().display(),
            command.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `line` as the `postgres` user, which is what the server tools require.
fn as_postgres(line: &str) -> Result<String> {
    run(Command::new("su").args([PG_USER, "-c", line]))
}

fn pg_bin() -> Result<String> {
    let listing = std::fs::read_dir("/usr/lib/postgresql").context("no PostgreSQL is installed")?;
    let mut versions: Vec<String> = listing
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    versions.sort();
    let version = versions
        .last()
        .context("/usr/lib/postgresql holds no version")?;
    Ok(format!("/usr/lib/postgresql/{version}/bin"))
}

impl Postgres {
    /// The destination selects the log filename extension read by the collector.
    pub(crate) fn start(destination: &str) -> Result<Self> {
        let bin = pg_bin()?;
        stop_previous(&bin);
        reset_directory(Path::new(PG_DATA))?;
        as_postgres(&format!(
            "{bin}/initdb --pgdata={PG_DATA} --auth=trust --username={PG_USER} --encoding=UTF8 \
             --no-sync"
        ))?;
        let extension = match destination {
            "csvlog" => "csv",
            "jsonlog" => "json",
            _ => "log",
        };
        append_config(
            &Path::new(PG_DATA).join("postgresql.conf"),
            &format!(
                "listen_addresses = '127.0.0.1'\n\
                 port = {PG_PORT}\n\
                 logging_collector = on\n\
                 log_destination = '{destination}'\n\
                 log_directory = 'log'\n\
                 log_filename = 'postgresql.log'\n\
                 log_rotation_age = 0\n\
                 log_rotation_size = 0\n\
                 log_checkpoints = on\n\
                 log_lock_waits = on\n\
                 log_temp_files = 0\n\
                 log_min_duration_statement = 0\n\
                 fsync = off\n"
            ),
        )?;
        as_postgres(&format!(
            "{bin}/pg_ctl --pgdata={PG_DATA} --wait --timeout=60 --log={PG_DATA}/startup.log start"
        ))?;
        Ok(Self {
            log_path: PathBuf::from(PG_DATA)
                .join("log")
                .join(format!("postgresql.{extension}")),
            dsn: format!(
                "host=127.0.0.1 port={PG_PORT} user={PG_USER} dbname={PG_USER} connect_timeout=5"
            ),
            psql: format!(
                "{bin}/psql --host=127.0.0.1 --port={PG_PORT} --username={PG_USER} \
                 --dbname={PG_USER}"
            ),
        })
    }

    /// Ignores SQL exit status so expected failures can be collected.
    pub(crate) fn statement(&self, sql: &str) -> Result<()> {
        let quoted = sql.replace('\'', r"'\''");
        Command::new("su")
            .args([
                PG_USER,
                "-c",
                &format!("{} --command '{quoted}'", self.psql),
            ])
            .output()
            .context("run psql")?;
        Ok(())
    }

    pub(crate) fn scalar(&self, sql: &str) -> Result<String> {
        let quoted = sql.replace('\'', r"'\''");
        let output = as_postgres(&format!(
            "{} --tuples-only --no-align --command '{quoted}'",
            self.psql
        ))?;
        Ok(output.trim().to_owned())
    }
}

impl PgBouncer {
    pub(crate) fn start() -> Result<Self> {
        let dir = Path::new(PGB_DIR);
        stop_previous_pooler(dir);
        reset_directory(dir)?;
        let port = next_pgb_port();
        std::fs::write(dir.join("users.txt"), format!("\"{PG_USER}\" \"\"\n"))
            .context("write the auth file")?;
        std::fs::write(
            dir.join("pgbouncer.ini"),
            format!(
                "[databases]\n\
                 {PG_USER} = host=127.0.0.1 port={PG_PORT} dbname={PG_USER}\n\
                 [pgbouncer]\n\
                 listen_addr = 127.0.0.1\n\
                 listen_port = {port}\n\
                 auth_type = trust\n\
                 auth_file = {PGB_DIR}/users.txt\n\
                 stats_users = {PG_USER}\n\
                 logfile = {PGB_DIR}/pgbouncer.log\n\
                 pidfile = {PGB_DIR}/pgbouncer.pid\n\
                 pool_mode = transaction\n"
            ),
        )
        .context("write the pooler configuration")?;
        // PgBouncer refuses to run as root.
        as_postgres(&format!("pgbouncer -d {PGB_DIR}/pgbouncer.ini"))?;
        // `-d` returns before the listener accepts connections.
        let ready = format!(
            "{}/psql --host=127.0.0.1 --port={port} --username={PG_USER} \
             --dbname=pgbouncer --command 'show version'",
            pg_bin()?
        );
        let mut answered = false;
        for _attempt in 0..100 {
            if as_postgres(&ready).is_ok() {
                answered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        anyhow::ensure!(answered, "the pooler never answered on its admin console");
        Ok(Self {
            dsn: format!("host=127.0.0.1 port={port} user={PG_USER} dbname=pgbouncer"),
            workload_dsn: format!(
                "host=127.0.0.1 port={port} user={PG_USER} dbname={PG_USER} connect_timeout=5"
            ),
            psql: format!(
                "{}/psql --host=127.0.0.1 --port={port} --username={PG_USER}",
                pg_bin()?
            ),
        })
    }

    pub(crate) fn connect_to(&self, database: &str) -> Result<()> {
        Command::new("su")
            .args([
                PG_USER,
                "-c",
                &format!("{} --dbname={database} --command 'select 1'", self.psql),
            ])
            .output()
            .context("run psql through the pooler")?;
        Ok(())
    }
}

fn append_config(path: &Path, settings: &str) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(settings.as_bytes())
        .with_context(|| format!("write {}", path.display()))
}

// Stop the previous server before removing its data directory.
fn stop_previous(bin: &str) {
    let _stopped = as_postgres(&format!(
        "{bin}/pg_ctl --pgdata={PG_DATA} --mode=immediate --wait --timeout=30 stop"
    ));
}

fn stop_previous_pooler(dir: &Path) {
    // Stop the last pooler before resetting its shared directory.
    if let Ok(pid) = std::fs::read_to_string(dir.join("pgbouncer.pid")) {
        let _killed = run(Command::new("kill").args(["-TERM", pid.trim()]));
    }
}

fn reset_directory(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    }
    std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    run(Command::new("chown").args(["-R", PG_USER, &path.to_string_lossy()]))?;
    Ok(())
}
