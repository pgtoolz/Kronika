use super::{parse_column, parse_header};
use syn::{DeriveInput, Field, parse_quote};

#[test]
fn sort_and_identity_keys_keep_their_declared_order() {
    let input: DeriveInput = parse_quote! {
        #[section(id = 1_100_001, name = "process", semantics = temporal,
                  sort_key("ts", "pid"), identity("pid", "starttime"))]
        struct Process {}
    };
    let header = parse_header(&input).unwrap();
    assert_eq!(header.id.base10_parse::<u32>().unwrap(), 1_100_001);
    assert_eq!(
        header
            .sort_key
            .iter()
            .map(syn::LitStr::value)
            .collect::<Vec<_>>(),
        ["ts", "pid"]
    );
    assert_eq!(
        header
            .identity
            .iter()
            .map(syn::LitStr::value)
            .collect::<Vec<_>>(),
        ["pid", "starttime"]
    );
}

#[test]
fn nullable_metric_keeps_its_unit_and_physical_type() {
    let field: Field = parse_quote! { #[column(g, unit = bytes)] memory: Option<i64> };
    let column = parse_column(&field).unwrap();
    assert!(column.nullable);
    assert_eq!(column.column_type, "I64");
    assert_eq!(column.column_class, "Gauge");
    assert_eq!(column.unit.unwrap(), "Bytes");
}

#[test]
fn missing_or_unknown_contract_attributes_are_rejected() {
    for (input, expected) in [
        (
            parse_quote!(
                #[section(name = "row", semantics = temporal)]
                struct Row {}
            ),
            "needs `id`",
        ),
        (
            parse_quote!(
                #[section(id = 1, name = "row", semantics = temporal, index("ts"))]
                struct Row {}
            ),
            "unknown #[section(..)] key",
        ),
    ] {
        assert!(
            parse_header(&input)
                .err()
                .unwrap()
                .to_string()
                .contains(expected)
        );
    }
}

#[test]
fn metric_columns_require_explicit_units() {
    let field: Field = parse_quote! { #[column(c)] calls: i64 };
    assert!(
        parse_column(&field)
            .err()
            .unwrap()
            .to_string()
            .contains("must declare `unit")
    );
}
