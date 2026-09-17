use super::query;

#[test]
fn query_uses_the_supplied_qualified_view() {
    let sql = query("\"metrics\".\"pg_stat_statements_info\"");
    assert!(sql.contains("FROM \"metrics\".\"pg_stat_statements_info\""));
    assert!(sql.contains("dealloc"));
    assert!(sql.contains("stats_reset_us"));
    assert!(sql.contains("kronika:"));
}
