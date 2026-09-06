const COMMERCE_TABLES: [&str; 8] = [
    "orders",
    "customers",
    "order_items",
    "products",
    "inventory",
    "payments",
    "event_log",
    "sessions",
];

pub(crate) const COMMERCE_TABLE_COUNT: u32 = 8;

#[cfg(test)]
pub(crate) const fn commerce_table_names() -> [&'static str; 8] {
    COMMERCE_TABLES
}

pub(crate) fn schema_name(schema: u32) -> String {
    if schema == 0 {
        "shop".to_owned()
    } else {
        format!("shop_{schema}")
    }
}

pub(crate) fn table_name(schema: u32, table: u32) -> String {
    let relation = usize::try_from(table)
        .ok()
        .and_then(|index| COMMERCE_TABLES.get(index))
        .map_or_else(|| format!("archive_{table}"), |name| (*name).to_owned());
    format!("{}.{relation}", schema_name(schema))
}

#[cfg(test)]
mod tests;
