use super::StatementScope;

#[test]
fn scope_parses_only_its_two_public_values() {
    assert_eq!(StatementScope::parse("all"), Some(StatementScope::All));
    assert_eq!(
        StatementScope::parse("workload"),
        Some(StatementScope::Workload)
    );
    assert_eq!(StatementScope::parse("Workload"), None);
    assert_eq!(StatementScope::parse(""), None);
    assert_eq!(StatementScope::All.as_str(), "all");
    assert_eq!(StatementScope::Workload.as_str(), "workload");
}

#[test]
fn only_the_workload_scope_filters_statements() {
    assert!(StatementScope::Workload.filters("pg_stat_statements"));
    assert!(!StatementScope::Workload.filters("pg_store_plans"));
    assert!(!StatementScope::All.filters("pg_stat_statements"));
}

#[test]
fn scopes_keep_row_and_series_routes_distinct() {
    assert!(StatementScope::Workload.allows_rows("pg_stat_statements"));
    assert!(!StatementScope::Workload.allows_rows("postgresql_summary"));
    assert!(StatementScope::Workload.allows_series(Some("postgresql_summary")));
    assert!(!StatementScope::Workload.allows_series(Some("pg_stat_statements")));
    assert!(!StatementScope::Workload.allows_series(None));
    assert!(StatementScope::All.allows_rows("os_cpu"));
    assert!(StatementScope::All.allows_series(None));
}

#[test]
fn collector_plan_keys_follow_the_statement_id_and_keep_database_and_role() {
    use super::statement_key;
    use kronika_reader::Row;
    use kronika_registry::Cell;
    for type_id in [1_002_001, 1_002_002, 1_003_001, 1_004_001, 1_018_001] {
        let contract = kronika_registry::contract(type_id).expect("statement or plan contract");
        for queryid in [0, -42, 71] {
            let cells = contract
                .columns
                .iter()
                .map(|column| match column.name {
                    "queryid" if type_id == 1_004_001 => Cell::I64(123),
                    "queryid_stat_statements" | "queryid" => Cell::I64(queryid),
                    "dbid" => Cell::U32(3),
                    "userid" => Cell::U32(7),
                    _ => Cell::Null,
                })
                .collect();
            let row = Row::new(contract, cells);
            assert_eq!(
                statement_key(&row),
                (queryid != 0).then_some([3, 7, queryid]),
                "{type_id}"
            );
        }
    }
}

#[test]
fn a_plan_without_any_statement_identity_operand_is_not_excluded() {
    use super::{plan_statement_query_id_columns, statement_key};
    use kronika_reader::Row;
    use kronika_registry::Cell;
    for type_id in [1_003_001, 1_004_001, 1_018_001] {
        let contract = kronika_registry::contract(type_id).expect("plan contract");
        for missing in [
            "dbid",
            "userid",
            plan_statement_query_id_columns(type_id)[0],
        ] {
            let cells = contract
                .columns
                .iter()
                .map(|column| match column.name {
                    name if name == missing => Cell::Null,
                    "dbid" | "userid" => Cell::U32(1),
                    "queryid" | "queryid_stat_statements" => Cell::I64(42),
                    _ => Cell::Null,
                })
                .collect();
            assert_eq!(
                statement_key(&Row::new(contract, cells)),
                None,
                "{type_id} {missing}"
            );
        }
    }
}
