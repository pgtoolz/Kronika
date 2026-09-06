//! Pure selection of index blocks from neutral segment metadata.

use kronika_reader::SegmentSection;
use kronika_registry::logical_section_name;

use crate::detect::finding_layout;
use crate::series::{SeriesKey, SeriesKind, pg_activity_layout, pg_database_layout};

/// Return every sparse-finding block described by neutral segment metadata.
#[must_use]
pub fn finding_keys_for_sections(sections: &[SegmentSection]) -> Vec<SeriesKey> {
    let mut keys = sections
        .iter()
        .filter(|section| finding_layout(section.type_id))
        .map(|section| SeriesKey {
            kind: SeriesKind::Findings,
            type_id: section.type_id,
        })
        .collect::<Vec<_>>();
    keys.push(SeriesKey {
        kind: SeriesKind::Findings,
        type_id: 0,
    });
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Return the allowlisted series described by neutral segment metadata.
#[must_use]
pub fn series_keys_for_sections(sections: &[SegmentSection], logical_name: &str) -> Vec<SeriesKey> {
    if logical_name == "health" {
        return vec![
            SeriesKey::OS_HEALTH,
            SeriesKey::OVERALL_HEALTH,
            SeriesKey::POSTGRES_HEALTH,
            SeriesKey {
                kind: SeriesKind::Findings,
                type_id: 0,
            },
        ];
    }
    let mut keys = Vec::new();
    for section in sections {
        match logical_name {
            "pg_stat_database" if pg_database_layout(section.type_id) => keys.push(SeriesKey {
                kind: SeriesKind::PgTransactionsPerSecond,
                type_id: section.type_id,
            }),
            "pg_stat_activity" if pg_activity_layout(section.type_id) => keys.push(SeriesKey {
                kind: SeriesKind::PgActiveBackends,
                type_id: section.type_id,
            }),
            _ => {}
        }
        if finding_layout(section.type_id)
            && logical_section_name(section.type_id).is_some_and(|name| name == logical_name)
        {
            keys.push(SeriesKey {
                kind: SeriesKind::Findings,
                type_id: section.type_id,
            });
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}
