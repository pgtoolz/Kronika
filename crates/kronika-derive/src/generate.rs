//! Emitting the contract, encoder and decoder for one section.

use super::{ColumnDef, Header, Ident, Span, TokenStream2, quote};

pub(super) fn build_contract(header: &Header, columns: &[ColumnDef]) -> TokenStream2 {
    let id = &header.id;
    let name = &header.name;
    let semantics_variant = semantics_variant(&header.semantics);
    let sort_key = &header.sort_key;
    let identity = &header.identity;

    let column_entries = columns.iter().map(|c| {
        let name = &c.name;
        let ty = &c.column_type;
        let class = &c.column_class;
        let nullable = c.nullable;
        let unit = c.unit.as_ref().map_or_else(
            || quote! { ::core::option::Option::None },
            |unit| quote! { ::core::option::Option::Some(::kronika_registry::Unit::#unit) },
        );
        quote! {
            ::kronika_registry::Column {
                name: #name,
                ty: ::kronika_registry::ColumnType::#ty,
                class: ::kronika_registry::ColumnClass::#class,
                nullable: #nullable,
                unit: #unit,
            }
        }
    });

    // `TypeId::new` runs in const context, so an invalid id fails compilation.
    quote! {
        const CONTRACT: ::kronika_registry::TypeContract = ::kronika_registry::TypeContract {
            type_id: match ::kronika_registry::TypeId::new(#id) {
                ::core::option::Option::Some(id) => id,
                ::core::option::Option::None => ::core::panic!(
                    "section type_id is invalid: unknown class, or a zero source or version"
                ),
            },
            name: #name,
            semantics: ::kronika_registry::Semantics::#semantics_variant,
            columns: &[ #( #column_entries ),* ],
            sort_key: &[ #( #sort_key ),* ],
            identity: &[ #( #identity ),* ],
            deprecated: false,
        };
    }
}

pub(super) fn semantics_variant(ident: &Ident) -> Ident {
    let name = ident.to_string();
    let variant = match name.as_str() {
        "snapshot_full" => "SnapshotFull",
        "conditional_full" => "ConditionalFull",
        "event_stream" => "EventStream",
        "changed" => "Changed",
        "on_change" => "OnChange",
        // Leave an unknown name as-is so rustc's error points at the enum.
        other => other,
    };
    Ident::new(variant, ident.span())
}

pub(super) fn build_encode(columns: &[ColumnDef]) -> TokenStream2 {
    // One builder per column. Collapsing these passes would require columnar
    // input; keep the row-slice API until benchmarks say otherwise.
    let builders = columns.iter().map(|c| {
        let field = &c.field;
        let name = &c.name;
        if c.column_type == "ListI32" {
            return quote! {
                ::kronika_registry::write_list_i32(#name, rows.iter().map(|r| r.#field.as_slice()))?
            };
        }
        let values = match (&c.wrapper, c.nullable) {
            (None, _) => quote! { rows.iter().map(|r| r.#field) },
            (Some(_), false) => quote! { rows.iter().map(|r| r.#field.0) },
            (Some(_), true) => quote! { rows.iter().map(|r| r.#field.map(|v| v.0)) },
        };
        match (&c.arrow_type, c.nullable) {
            (Some(at), false) => quote! {
                ::kronika_registry::write_required::<::kronika_registry::#at>(#values)
            },
            (Some(at), true) => quote! {
                ::kronika_registry::write_nullable::<::kronika_registry::#at>(#values)
            },
            (None, false) => quote! { ::kronika_registry::write_bool(#values) },
            (None, true) => quote! { ::kronika_registry::write_bool_nullable(#values) },
        }
    });
    quote! { ::std::vec![ #( #builders ),* ] }
}

/// Generate `Section::ts_range` from the non-nullable `#[column(t)]` field.
pub(super) fn build_ts_range(columns: &[ColumnDef]) -> TokenStream2 {
    columns
        .iter()
        .find(|column| column.column_class == "Timestamp" && !column.nullable)
        .map_or_else(
            || {
                quote! {
                    fn ts_range(_rows: &[Self]) -> ::core::option::Option<(i64, i64)> {
                        ::core::option::Option::None
                    }
                }
            },
            |column| {
                let field = &column.field;
                quote! {
                    fn ts_range(rows: &[Self]) -> ::core::option::Option<(i64, i64)> {
                        let mut values = rows.iter().map(|row| row.#field.0);
                        let first = values.next()?;
                        ::core::option::Option::Some(
                            values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))),
                        )
                    }
                }
            },
        )
}

/// Generate conservative accounting for child values in every `ListI32`
/// column. Saturation makes an unrepresentable total fail closed in admission
/// code instead of wrapping to an underestimated value.
pub(super) fn build_list_i32_child_value_count(columns: &[ColumnDef]) -> TokenStream2 {
    let fields: Vec<&Ident> = columns
        .iter()
        .filter(|column| column.column_type == "ListI32")
        .map(|column| &column.field)
        .collect();
    if fields.is_empty() {
        return quote! {
            fn list_i32_child_value_count(_rows: &[Self]) -> usize {
                0
            }
        };
    }

    let additions = fields.iter().map(|field| {
        quote! {
            count = count.saturating_add(row.#field.len());
        }
    });
    quote! {
        fn list_i32_child_value_count(rows: &[Self]) -> usize {
            rows.iter().fold(0usize, |mut count, row| {
                #( #additions )*
                count
            })
        }
    }
}

pub(super) fn build_decode(struct_name: &Ident, columns: &[ColumnDef]) -> TokenStream2 {
    // Mixed-site idents avoid collisions with user fields and in-scope tuple
    // structs such as `Ts` and `StrId`.
    let batch = Ident::new("batch", Span::mixed_site());
    let out = Ident::new("out", Span::mixed_site());
    let idx = Ident::new("i", Span::mixed_site());
    let cols: Vec<Ident> = (0..columns.len())
        .map(|n| Ident::new(&format!("col{n}"), Span::mixed_site()))
        .collect();

    let bindings = columns.iter().zip(&cols).map(|(c, col)| {
        let name = &c.name;
        if c.column_type == "ListI32" {
            return quote! { let #col = ::kronika_registry::read_list_i32(#batch, #name)?; };
        }
        match (&c.arrow_type, c.nullable) {
            // Required primitive: rebind to the values slice, so the row loop
            // gathers by `slice[i]` (one bounds-check the optimizer can hoist)
            // instead of `PrimitiveArray::value(i)` per cell.
            (Some(at), false) => quote! {
                let #col = ::kronika_registry::required_column::<::kronika_registry::#at>(#batch, #name)?;
                let #col = #col.values();
            },
            // Nullable arrays stay intact so `opt_primitive` can check nulls.
            (Some(at), true) => quote! {
                let #col = ::kronika_registry::nullable_column::<::kronika_registry::#at>(#batch, #name)?;
            },
            (None, false) => quote! {
                let #col = ::kronika_registry::required_bool(#batch, #name)?;
            },
            (None, true) => quote! {
                let #col = ::kronika_registry::nullable_bool(#batch, #name)?;
            },
        }
    });

    let cells = columns.iter().zip(&cols).map(|(c, col)| {
        let field = &c.field;
        if c.column_type == "ListI32" {
            return quote! { #field: #col.value(#idx) };
        }
        let value = match (&c.wrapper, &c.arrow_type, c.nullable) {
            (Some(w), _, false) => quote! { ::kronika_registry::#w(#col[#idx]) },
            (Some(w), _, true) => quote! {
                ::kronika_registry::opt_primitive(#col, #idx).map(::kronika_registry::#w)
            },
            (None, Some(_), false) => quote! { #col[#idx] },
            (None, None, false) => quote! { #col.value(#idx) },
            (None, Some(_), true) => quote! { ::kronika_registry::opt_primitive(#col, #idx) },
            (None, None, true) => quote! { ::kronika_registry::opt_bool(#col, #idx) },
        };
        quote! { #field: #value }
    });

    quote! {
        ::kronika_registry::decode_section(&Self::CONTRACT, section, |#batch, #out| {
            #( #bindings )*
            for #idx in 0..#batch.num_rows() {
                #out.push(#struct_name { #( #cells ),* });
            }
            ::core::result::Result::Ok(())
        })
    }
}
