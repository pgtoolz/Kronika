//! Reading the `#[section]` and `#[column]` attributes off the struct.

use super::{ColumnDef, DeriveInput, Header, Ident, LitInt, LitStr, Span, Spanned, Token, Type};

pub(super) fn parse_header(input: &DeriveInput) -> syn::Result<Header> {
    let attr = input
        .attrs
        .iter()
        .find(|a| a.path().is_ident("section"))
        .ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "Section requires a #[section(..)] header",
            )
        })?;

    let mut id = None;
    let mut name = None;
    let mut semantics = None;
    let mut sort_key = Vec::new();
    let mut identity = Vec::new();

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse::<LitInt>()?);
        } else if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?);
        } else if meta.path.is_ident("semantics") {
            semantics = Some(meta.value()?.parse::<Ident>()?);
        } else if meta.path.is_ident("sort_key") {
            let content;
            syn::parenthesized!(content in meta.input);
            let keys = content.parse_terminated(<LitStr as syn::parse::Parse>::parse, Token![,])?;
            sort_key = keys.into_iter().collect();
        } else if meta.path.is_ident("identity") {
            let content;
            syn::parenthesized!(content in meta.input);
            let keys = content.parse_terminated(<LitStr as syn::parse::Parse>::parse, Token![,])?;
            identity = keys.into_iter().collect();
        } else {
            return Err(meta.error("unknown #[section(..)] key"));
        }
        Ok(())
    })?;

    Ok(Header {
        id: id.ok_or_else(|| syn::Error::new(attr.span(), "#[section(..)] needs `id`"))?,
        name: name.ok_or_else(|| syn::Error::new(attr.span(), "#[section(..)] needs `name`"))?,
        semantics: semantics
            .ok_or_else(|| syn::Error::new(attr.span(), "#[section(..)] needs `semantics`"))?,
        sort_key,
        identity,
    })
}

pub(super) fn parse_column(field: &syn::Field) -> syn::Result<ColumnDef> {
    let field_ident = field
        .ident
        .clone()
        .ok_or_else(|| syn::Error::new(field.span(), "field needs a name"))?;

    let class_attr = field
        .attrs
        .iter()
        .find(|a| a.path().is_ident("column"))
        .ok_or_else(|| syn::Error::new(field.span(), "field needs a #[column(class)] attribute"))?;
    let args: ColumnArgs = class_attr.parse_args()?;
    let column_class = column_class(&args.class)?;
    let unit = args.unit;

    let (inner, nullable) = unwrap_option(&field.ty);

    // `Vec<i32>` is not a bare ident, so it has its own branch: a list column is
    // never NULL (an empty vec is an empty list) and needs no Arrow scalar type
    // or wrapper.
    if is_vec_i32(inner) {
        return Ok(ColumnDef {
            name: field_ident.to_string(),
            field: field_ident,
            column_type: Ident::new("ListI32", Span::call_site()),
            column_class,
            arrow_type: None,
            wrapper: None,
            nullable: false,
            unit,
        });
    }

    let inner_ident = type_ident(inner)?;
    let (column_type, arrow_type, wrapper) = map_type(&inner_ident, &column_class)?;

    Ok(ColumnDef {
        name: field_ident.to_string(),
        field: field_ident,
        column_type,
        column_class,
        arrow_type,
        wrapper,
        nullable,
        unit,
    })
}

/// Arguments of `#[column(...)]`.
pub(super) struct ColumnArgs {
    class: Ident,
    unit: Option<Ident>,
}

impl syn::parse::Parse for ColumnArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let class: Ident = input.parse()?;
        let mut unit = None;
        while input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            let key: Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            if key != "unit" {
                return Err(syn::Error::new(key.span(), "expected `unit`"));
            }
            if unit.is_some() {
                return Err(syn::Error::new(key.span(), "duplicate `unit`"));
            }
            let value: Ident = input.parse()?;
            unit = Some(unit_variant(&value)?);
        }
        // A counter or gauge without a declared unit is a number nobody can
        // read, so the macro refuses it rather than defaulting.
        if matches!(class.to_string().as_str(), "c" | "g") && unit.is_none() {
            return Err(syn::Error::new(
                class.span(),
                "a counter or gauge column must declare `unit = ...`; write `unit = none` when it counts bare occurrences",
            ));
        }
        Ok(Self { class, unit })
    }
}

/// Maps the attribute spelling of a unit to its `Unit` variant.
pub(super) fn unit_variant(value: &Ident) -> syn::Result<Ident> {
    let name = match value.to_string().as_str() {
        "none" => "None",
        "count" => "Count",
        "bytes" => "Bytes",
        "kib" => "Kib",
        "pages" => "Pages",
        "sectors" => "Sectors",
        "seconds" => "Seconds",
        "milliseconds" => "Milliseconds",
        "microseconds" => "Microseconds",
        "nanoseconds" => "Nanoseconds",
        "jiffies" => "Jiffies",
        "hertz" => "Hertz",
        "megabits_per_second" => "MegabitsPerSecond",
        "megabytes_per_second" => "MegabytesPerSecond",
        "percent" => "Percent",
        "celsius" => "Celsius",
        other => {
            return Err(syn::Error::new(
                value.span(),
                format!("unknown unit `{other}`"),
            ));
        }
    };
    Ok(Ident::new(name, value.span()))
}

pub(super) fn column_class(ident: &Ident) -> syn::Result<Ident> {
    let variant = match ident.to_string().as_str() {
        "c" => "Cumulative",
        "g" => "Gauge",
        "l" => "Label",
        "t" => "Timestamp",
        _ => {
            return Err(syn::Error::new(
                ident.span(),
                "column class must be one of c (cumulative), g (gauge), l (label), t (timestamp)",
            ));
        }
    };
    Ok(Ident::new(variant, ident.span()))
}

/// Map a field's base-type ident and class to its `ColumnType`, Arrow type
/// token, and optional wrapper (`Ts` or `StrId`).
pub(super) fn map_type(
    ident: &Ident,
    class: &Ident,
) -> syn::Result<(Ident, Option<Ident>, Option<Ident>)> {
    let span = ident.span();
    let is_timestamp = class == "Timestamp";

    let (column_type, arrow_type, wrapper): (&str, Option<&str>, Option<&str>) = match ident
        .to_string()
        .as_str()
    {
        "i8" => ("I8", Some("Int8Type"), None),
        "i16" => ("I16", Some("Int16Type"), None),
        "i32" => ("I32", Some("Int32Type"), None),
        "i64" => ("I64", Some("Int64Type"), None),
        "u8" => ("U8", Some("UInt8Type"), None),
        "u16" => ("U16", Some("UInt16Type"), None),
        "u32" => ("U32", Some("UInt32Type"), None),
        "u64" => ("U64", Some("UInt64Type"), None),
        "f32" => ("F32", Some("Float32Type"), None),
        "f64" => ("F64", Some("Float64Type"), None),
        "bool" => ("Bool", None, None),
        "Ts" => ("Ts", Some("Int64Type"), Some("Ts")),
        "StrId" => ("StrId", Some("UInt64Type"), Some("StrId")),
        other => {
            return Err(syn::Error::new(
                span,
                format!(
                    "unsupported column type `{other}`; expected a base type like i64, u32, f64, bool, Ts, StrId"
                ),
            ));
        }
    };

    if is_timestamp && column_type != "Ts" {
        return Err(syn::Error::new(
            span,
            "a column of class t (timestamp) must be a `Ts`",
        ));
    }

    Ok((
        Ident::new(column_type, span),
        arrow_type.map(|at| Ident::new(at, span)),
        wrapper.map(|w| Ident::new(w, span)),
    ))
}

/// Split `Option<T>` into `(T, true)`; a non-option type into `(ty, false)`.
pub(super) fn unwrap_option(ty: &Type) -> (&Type, bool) {
    if let Type::Path(path) = ty
        && path.qself.is_none()
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return (inner, true);
    }
    (ty, false)
}

/// True for a `Vec<i32>` field type.
pub(super) fn is_vec_i32(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Vec" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    matches!(
        args.args.first(),
        Some(syn::GenericArgument::Type(inner))
            if type_ident(inner).is_ok_and(|ident| ident == "i32")
    )
}

/// The single path-segment ident of a simple type like `i64`.
pub(super) fn type_ident(ty: &Type) -> syn::Result<Ident> {
    if let Type::Path(path) = ty
        && path.qself.is_none()
        && let Some(segment) = path.path.segments.last()
        && segment.arguments.is_empty()
    {
        return Ok(segment.ident.clone());
    }
    Err(syn::Error::new(ty.span(), "expected a simple base type"))
}
