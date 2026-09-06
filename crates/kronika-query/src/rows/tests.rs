use super::{Cursor, binding};
use crate::request::{DataRequest, Filter, Order, RowsRequest, SegmentRequest};

fn request(order: Order) -> RowsRequest {
    RowsRequest {
        data: DataRequest {
            segment: SegmentRequest {
                segment_id: 7,
                section: "os_process".to_owned(),
            },
            fields: vec!["pid".to_owned(), "rss_bytes".to_owned()],
            filters: vec![Filter {
                column: "pid".to_owned(),
                value: "42".to_owned(),
            }],
            type_id: None,
            after: None,
        },
        order,
        page_size: 100,
        cursor: None,
    }
}

#[test]
fn cursor_roundtrips_every_physical_coordinate() {
    let cursor = Cursor {
        segment_id: i64::MAX,
        active_position: u64::MAX,
        layout_index: 17,
        position: u64::MAX - 1,
        binding: u64::MAX - 2,
    };
    assert_eq!(Cursor::parse(&cursor.encode()).expect("cursor"), cursor);
}

#[test]
fn malformed_cursor_is_rejected() {
    assert!(Cursor::parse("7,8,9").is_err());
    assert!(Cursor::parse("7,8,layout,10,11").is_err());
}

#[test]
fn binding_changes_with_order_projection_and_filters() {
    let asc = request(Order::Asc);
    let mut changed = asc.clone();
    changed.order = Order::Desc;
    assert_ne!(binding(&asc), binding(&changed));

    changed = asc.clone();
    changed.data.fields.swap(0, 1);
    assert_ne!(binding(&asc), binding(&changed));

    changed = asc.clone();
    changed.data.filters[0].value = "43".to_owned();
    assert_ne!(binding(&asc), binding(&changed));

    changed = asc.clone();
    changed.data.type_id = Some(1_100_001);
    assert_ne!(binding(&asc), binding(&changed));
}
