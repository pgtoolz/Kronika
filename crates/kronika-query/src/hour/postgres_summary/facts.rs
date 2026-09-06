use kronika_reader::{Cell, Row};

pub(super) const FIELDS: &str = "active_count active_pct mean_exec_ms buffer_read_pct wal_bytes_per_call calls_per_active rollback_pct temp_bytes_per_transaction seq_scan_pct hot_update_pct dead_tuple_pct vacuumed_pct toast_pct xid_boundary_pct scanned_pct no_scan_pct usable_pct";

pub(super) const WANTED: &str = "calls total_exec_time total_time shared_blks_read shared_blks_hit local_blks_read local_blks_hit wal_bytes datid xact_commit xact_rollback temp_bytes blks_read blks_hit seq_scan idx_scan n_tup_upd n_tup_hot_upd n_live_tup n_dead_tup vacuum_count autovacuum_count main_fork_bytes toast_bytes heap_blks_read heap_blks_hit idx_blks_read idx_blks_hit toast_blks_read toast_blks_hit tidx_blks_read tidx_blks_hit xid_age indisvalid indisready";
const DENSE_PREVIOUS: &str = "calls total_exec_time total_time shared_blks_read shared_blks_hit local_blks_read local_blks_hit wal_bytes";
const DB_PREVIOUS: &str = "xact_commit xact_rollback temp_bytes blks_read blks_hit";
const TABLE_PREVIOUS: &str = "seq_scan idx_scan n_tup_upd n_tup_hot_upd vacuum_count autovacuum_count heap_blks_read heap_blks_hit idx_blks_read idx_blks_hit toast_blks_read toast_blks_hit tidx_blks_read tidx_blks_hit";
const INDEX_PREVIOUS: &str = "idx_scan idx_blks_read idx_blks_hit";
const TABLE_READS: &str = "heap_blks_read idx_blks_read toast_blks_read tidx_blks_read";
const TABLE_HITS: &str = "heap_blks_hit idx_blks_hit toast_blks_hit tidx_blks_hit";

pub(super) enum Previous {
    Dense(Box<[Option<Cell>; 8]>),
    Database(Box<[Option<Cell>; 5]>),
    Table(Box<[Option<Cell>; 14]>),
    Index(Box<[Option<Cell>; 3]>),
}

impl Previous {
    pub(super) fn new(surface: u8, row: &Row) -> Self {
        let cell = |fields: &str, at| row.get(fields.split_ascii_whitespace().nth(at)?).cloned();
        match surface {
            1 | 2 => Self::Dense(Box::new(std::array::from_fn(|at| cell(DENSE_PREVIOUS, at)))),
            3 => Self::Database(Box::new(std::array::from_fn(|at| cell(DB_PREVIOUS, at)))),
            4 => Self::Table(Box::new(std::array::from_fn(|at| cell(TABLE_PREVIOUS, at)))),
            5 => Self::Index(Box::new(std::array::from_fn(|at| cell(INDEX_PREVIOUS, at)))),
            _ => unreachable!("fixed PostgreSQL summary surface"),
        }
    }

    fn get(&self, name: &str) -> Option<&Cell> {
        let (fields, values): (&str, &[Option<Cell>]) = match self {
            Self::Dense(values) => (DENSE_PREVIOUS, values.as_slice()),
            Self::Database(values) => (DB_PREVIOUS, values.as_slice()),
            Self::Table(values) => (TABLE_PREVIOUS, values.as_slice()),
            Self::Index(values) => (INDEX_PREVIOUS, values.as_slice()),
        };
        let at = fields
            .split_ascii_whitespace()
            .position(|value| value == name)?;
        values[at].as_ref()
    }
}

#[derive(Default)]
pub(super) struct Totals([Option<f64>; 12]);

impl Totals {
    fn add(&mut self, at: usize, value: Option<f64>) {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            self.0[at] = Some(self.0[at].unwrap_or(0.0) + value);
        }
    }

    fn get(&self, at: usize) -> Option<f64> {
        self.0[at].filter(|value| value.is_finite())
    }

    fn pair(&mut self, at: usize, values: Option<(f64, f64)>) {
        if let Some((left, right)) = values {
            self.add(at, Some(left));
            self.add(at + 1, Some(right));
        }
    }

    fn merge(&mut self, other: &Self) {
        for (at, value) in other.0.iter().copied().enumerate() {
            self.add(at, value);
        }
    }
}

pub(super) enum Summary {
    Dense(f64, Totals),
    Database(Totals),
    Table(f64, f64, Totals),
    Index(f64, Totals),
}

impl Summary {
    pub(super) fn new(surface: u8) -> Self {
        match surface {
            1 | 2 => Self::Dense(0.0, Totals::default()),
            3 => Self::Database(Totals::default()),
            4 => Self::Table(0.0, 0.0, Totals::default()),
            5 => Self::Index(0.0, Totals::default()),
            _ => unreachable!("fixed PostgreSQL summary surface"),
        }
    }

    pub(super) fn add(&mut self, row: &Row, before: Option<&Previous>) {
        let flag = |value| f64::from(u8::from(value));
        match self {
            Self::Dense(total, sums) => {
                let calls = delta(row, before, "calls");
                *total += flag(calls.is_some());
                sums.add(0, calls.map(|value| flag(value > 0.0)));
                sums.add(1, calls);
                let execution = row
                    .get("total_exec_time")
                    .map_or("total_time", |_| "total_exec_time");
                sums.pair(2, delta(row, before, execution).zip(calls));
                let reads = deltas(row, before, "shared_blks_read local_blks_read");
                let buffers = "shared_blks_read shared_blks_hit local_blks_read local_blks_hit";
                sums.pair(4, reads.zip(deltas(row, before, buffers)));
                sums.pair(6, delta(row, before, "wal_bytes").zip(calls));
            }
            Self::Database(sums) => {
                let tx = deltas(row, before, "xact_commit xact_rollback");
                sums.pair(0, delta(row, before, "xact_rollback").zip(tx));
                sums.pair(2, delta(row, before, "temp_bytes").zip(tx));
                sums.pair(
                    4,
                    delta(row, before, "blks_read").zip(deltas(row, before, "blks_read blks_hit")),
                );
            }
            Self::Table(total, xid_total, sums) => {
                let scans =
                    delta(row, before, "seq_scan").zip(optional_delta(row, before, "idx_scan"));
                sums.pair(0, scans.map(|(seq, idx)| (seq, seq + idx)));
                sums.pair(
                    2,
                    delta(row, before, "n_tup_hot_upd").zip(delta(row, before, "n_tup_upd")),
                );
                let tuples = number(row, "n_live_tup").zip(number(row, "n_dead_tup"));
                sums.pair(4, tuples.map(|(live, dead)| (dead, live + dead)));
                let vacuumed = deltas(row, before, "vacuum_count autovacuum_count");
                *total += flag(vacuumed.is_some());
                sums.add(6, vacuumed.map(|value| flag(value > 0.0)));
                let main = number(row, "main_fork_bytes");
                let toast = number(row, "toast_bytes").or(Some(0.0));
                sums.pair(7, toast.zip(main.zip(toast).map(|(a, b)| a + b)));
                let reads = optional_deltas(row, before, TABLE_READS);
                let hits = optional_deltas(row, before, TABLE_HITS);
                sums.pair(9, reads.zip(reads.zip(hits).map(|(a, b)| a + b)));
                if let Some(age) = number(row, "xid_age") {
                    *xid_total += 1.0;
                    sums.add(11, Some(flag(age >= 1_600_000_000.0)));
                }
            }
            Self::Index(total, sums) => {
                let scans = delta(row, before, "idx_scan");
                *total += flag(scans.is_some());
                sums.add(0, scans.map(|value| flag(value > 0.0)));
                sums.add(1, scans.map(|value| flag(value == 0.0)));
                sums.pair(
                    2,
                    delta(row, before, "idx_blks_read").zip(deltas(
                        row,
                        before,
                        "idx_blks_read idx_blks_hit",
                    )),
                );
                let usable = match (row.get("indisvalid"), row.get("indisready")) {
                    (Some(Cell::Bool(valid)), Some(Cell::Bool(ready))) => Some(*valid && *ready),
                    _ => None,
                };
                sums.add(4, usable.map(flag));
                sums.add(5, usable.map(|_| 1.0));
            }
        }
    }

    pub(super) fn merge(&mut self, other: &Self) {
        match (self, other) {
            (Self::Table(total, xid_total, sums), Self::Table(b, x, s)) => {
                *total += b;
                *xid_total += x;
                sums.merge(s);
            }
            (Self::Index(total, sums), Self::Index(b, s)) => {
                *total += b;
                sums.merge(s);
            }
            _ => {}
        }
    }

    pub(super) fn values(&self, surface: u8) -> [Option<f64>; 17] {
        let mut out = [None; 17];
        match self {
            Self::Dense(total, sums) => {
                out[0] = sums.get(0);
                out[1] = pct(sums.get(0), Some(*total));
                out[2] = ratio(sums.get(2), sums.get(3));
                out[3] = pct(sums.get(4), sums.get(5));
                if surface == 1 {
                    out[4] = ratio(sums.get(6), sums.get(7));
                } else {
                    out[5] = ratio(sums.get(1), sums.get(0));
                }
            }
            Self::Database(sums) => {
                out[6] = pct(sums.get(0), sums.get(1));
                out[7] = ratio(sums.get(2), sums.get(3));
                out[3] = pct(sums.get(4), sums.get(5));
            }
            Self::Table(total, xid_total, sums) => {
                out[8] = pct(sums.get(0), sums.get(1));
                out[9] = pct(sums.get(2), sums.get(3));
                out[10] = pct(sums.get(4), sums.get(5));
                out[11] = pct(sums.get(6), Some(*total));
                out[12] = pct(sums.get(7), sums.get(8));
                out[3] = pct(sums.get(9), sums.get(10));
                out[13] = pct(sums.get(11), Some(*xid_total));
            }
            Self::Index(total, sums) => {
                out[14] = pct(sums.get(0), Some(*total));
                out[15] = pct(sums.get(1), Some(*total));
                out[3] = pct(sums.get(2), sums.get(3));
                out[16] = pct(sums.get(4), sums.get(5));
            }
        }
        out
    }
}

fn ratio(part: Option<f64>, total: Option<f64>) -> Option<f64> {
    let total = total.filter(|total| *total > 0.0)?;
    part.map(|part| part / total)
}

fn pct(part: Option<f64>, total: Option<f64>) -> Option<f64> {
    ratio(part, total).map(|value| 100.0 * value)
}

fn number(row: &Row, name: &str) -> Option<f64> {
    match row.get(name) {
        Some(Cell::F64(value)) => Some(*value),
        cell => integer(cell).map(float),
    }
}

pub(super) fn integer(cell: Option<&Cell>) -> Option<i128> {
    match cell {
        Some(Cell::I16(v)) => Some(i128::from(*v)),
        Some(Cell::I32(v)) => Some(i128::from(*v)),
        Some(Cell::I64(v) | Cell::Ts(v)) => Some(i128::from(*v)),
        Some(Cell::U32(v)) => Some(i128::from(*v)),
        Some(Cell::U64(v) | Cell::StrId(v)) => Some(i128::from(*v)),
        Some(Cell::Bool(v)) => Some(i128::from(*v)),
        _ => None,
    }
}

fn delta(row: &Row, before: Option<&Previous>, name: &str) -> Option<f64> {
    let before = before?;
    let value = match (row.get(name), before.get(name)) {
        (Some(Cell::F64(current)), Some(Cell::F64(previous))) => current - previous,
        (current, previous) => float(integer(current)?.checked_sub(integer(previous)?)?),
    };
    (value >= 0.0 && value.is_finite()).then_some(value)
}

fn deltas(row: &Row, before: Option<&Previous>, names: &str) -> Option<f64> {
    names
        .split_ascii_whitespace()
        .try_fold(0.0, |sum, name| Some(sum + delta(row, before, name)?))
}

fn optional_delta(row: &Row, before: Option<&Previous>, name: &str) -> Option<f64> {
    let before = before?;
    (matches!(row.get(name), Some(Cell::Null)) && matches!(before.get(name), Some(Cell::Null)))
        .then_some(0.0)
        .or_else(|| delta(row, Some(before), name))
}

fn optional_deltas(row: &Row, before: Option<&Previous>, names: &str) -> Option<f64> {
    names.split_ascii_whitespace().try_fold(0.0, |sum, name| {
        Some(sum + optional_delta(row, before, name)?)
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "exact counter differences become floating point for summary division"
)]
const fn float(value: i128) -> f64 {
    value as f64
}
