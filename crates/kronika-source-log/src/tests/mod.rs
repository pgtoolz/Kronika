pub(crate) fn utc_micros(text: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .expect("wall clock")
        .and_utc()
        .timestamp_micros()
}
