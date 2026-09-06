//! Internal derive for a registry section contract and Parquet codec.
//!
//! `#[derive(Section)]` accepts a named-field struct with one
//! `#[section(id = ..., name = ..., semantics = ..., sort_key(...),
//! identity(...))]` attribute. Every field needs `#[column(class)]`; a column
//!
//! The generated finished `kronika_registry::Section` implementation exposes
//! one `TypeContract`, encodes at most `MAX_SECTION_ROWS`, decodes only a
//! CRC-verified section, and derives its timestamp range from a field named
//! `ts`. Registry linting validates references and semantic invariants across
//! the complete contract set.
//!
//! Supported field spellings are the registry primitive integer and float
//! widths, `bool`, `Ts`, `StrId`, `Vec<i32>`, and `Option<T>` for nullable
//! scalars. Types are matched by their written identifiers because a proc
//! macro receives tokens, not resolved Rust types. Unsupported shapes and
//! attributes fail at compile time with a span-local error.
//!
//! This proc macro is an implementation detail of `kronika-registry`; it is
//! not the extension point for downstream section types.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, Ident, LitInt, LitStr, Token, Type, parse_macro_input};

/// Derive the section contract and Parquet codec for a typed struct.
///
/// See the crate docs for the attribute grammar.
mod generate;
mod parse;

use generate::{
    build_contract, build_decode, build_encode, build_list_i32_child_value_count, build_ts_range,
};
use parse::{parse_column, parse_header};

/// Generates the section contract, encoder and decoder for one row struct.
#[proc_macro_derive(Section, attributes(section, column))]
pub fn derive_section(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Collected `#[section(..)]` header values.
struct Header {
    id: LitInt,
    name: LitStr,
    semantics: Ident,
    sort_key: Vec<LitStr>,
    identity: Vec<LitStr>,
}

/// One resolved column: its field, on-disk shape, and class.
struct ColumnDef {
    field: Ident,
    name: String,
    /// `ColumnType` variant ident, e.g. `I64`, `Ts`.
    column_type: Ident,
    /// `ColumnClass` variant ident, e.g. `Cumulative`.
    column_class: Ident,
    /// Arrow primitive type token for the shared helpers, or `None` for
    /// `bool` (which uses the dedicated boolean helpers).
    arrow_type: Option<Ident>,
    /// Wrapper over the Arrow native value (`Ts`, `StrId`), or `None` when the
    /// field already is the native type. Encode reads `.0`; decode wraps the
    /// decoded value back into it.
    wrapper: Option<Ident>,
    nullable: bool,
    /// Declared unit, or `None` for a label or timestamp column.
    unit: Option<Ident>,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let header = parse_header(input)?;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "Section requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                Span::call_site(),
                "Section can only be derived for a struct",
            ));
        }
    };

    let columns: Vec<ColumnDef> = fields
        .iter()
        .map(parse_column)
        .collect::<syn::Result<_>>()?;

    let struct_name = &input.ident;
    let contract = build_contract(&header, &columns);
    let encode = build_encode(&columns);
    let decode = build_decode(struct_name, &columns);
    let ts_range = build_ts_range(&columns);
    let list_i32_child_value_count = build_list_i32_child_value_count(&columns);

    Ok(quote! {
        impl ::kronika_registry::private::Private for #struct_name {}

        impl ::kronika_registry::Section for #struct_name {
            #contract

            fn encode(rows: &[Self]) -> ::core::result::Result<
                ::std::vec::Vec<u8>,
                ::kronika_registry::CodecError,
            > {
                // Reject over-cap input before building Arrow arrays.
                ::kronika_registry::check_row_cap(rows.len())?;
                let columns = #encode;
                ::kronika_registry::encode_section(&Self::CONTRACT, columns)
            }

            fn decode(section: ::kronika_registry::VerifiedSection) -> ::core::result::Result<
                ::std::vec::Vec<Self>,
                ::kronika_registry::CodecError,
            > {
                #decode
            }

            #ts_range

            #list_i32_child_value_count
        }
    })
}
