use std::borrow::Cow;

use super::{MIN_FIELDS, continues, split};

#[test]
fn quoted_fields_keep_their_commas_and_doubled_quotes() {
    let fields = split(r#"a,"b,c","say ""hi""",,d"#);

    assert_eq!(fields, ["a", "b,c", r#"say "hi""#, "", "d"]);
    assert!(matches!(fields.first(), Some(Cow::Borrowed("a"))));
    assert!(matches!(fields.get(1), Some(Cow::Owned(_))));
}

#[test]
fn appended_columns_are_not_split_after_the_supported_record() {
    let record = (0..MIN_FIELDS + 2)
        .map(|field| field.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let fields = split(&record);

    assert_eq!(fields.len(), MIN_FIELDS);
    assert_eq!(fields.last().expect("last supported field").as_ref(), "22");
}

#[test]
fn a_record_continues_while_a_quoted_field_is_open() {
    let open = vec![r#"2026-08-07 12:34:56.789 MSK,"alice","shop",1,"#.to_owned()];
    assert!(!continues(&open, "next line", false));

    let open = vec![r#"...,"select 1"#.to_owned()];
    assert!(continues(&open, "from t", true));
}
