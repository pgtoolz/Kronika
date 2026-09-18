use arrow_array::ArrayRef;
use arrow_array::array::new_empty_array;
use bytes::Bytes;

use super::{
    BytesPool, CodecError, ColumnClass, ColumnType, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, Unit,
    VerifiedSection, arrow_schema, decode_any, decode_pooled, encode_section, lint_registry,
    registry, section_name,
};

#[test]
fn the_registry_is_clean() {
    assert_eq!(lint_registry(), Ok(()));
}

#[test]
fn registry_is_not_empty() {
    assert!(!registry().is_empty());
}

#[test]
fn every_timestamp_gauge_is_measured_in_microseconds() {
    for contract in registry() {
        for column in contract
            .columns
            .iter()
            .filter(|column| column.class == ColumnClass::Gauge && column.ty == ColumnType::Ts)
        {
            assert_eq!(
                column.unit,
                Some(Unit::Microseconds),
                "type {} column {}",
                contract.type_id.get(),
                column.name
            );
        }
    }
}

/// Registry wiring check with no per-type branch.
#[test]
fn every_registered_type_decodes_an_empty_section() {
    for contract in registry() {
        let schema = arrow_schema(contract);
        let columns: Vec<ArrayRef> = schema
            .fields()
            .iter()
            .map(|field| new_empty_array(field.data_type()))
            .collect();
        let bytes = encode_section(contract, columns).expect("encode empty section");
        let bytes_in = bytes.len();
        let id = contract.type_id.get();
        let decoded = decode_any(id, VerifiedSection::for_test(bytes.into())).expect("decode_any");
        assert_eq!(decoded.stats.type_id, id, "stats carries the type_id");
        assert_eq!(
            decoded.stats.rows, 0,
            "type {id} decoded a non-empty section"
        );
        assert_eq!(
            decoded.stats.bytes_in, bytes_in,
            "stats.bytes_in matches input"
        );
    }
}

#[test]
fn decode_pooled_reads_into_a_pooled_buffer_and_decodes() {
    let pool = BytesPool::new(2, 1 << 20);
    let contract = &registry()[0];
    let id = contract.type_id.get();
    let columns: Vec<ArrayRef> = arrow_schema(contract)
        .fields()
        .iter()
        .map(|field| new_empty_array(field.data_type()))
        .collect();
    let encoded = encode_section(contract, columns).expect("encode empty section");
    // Stand-in CRC = byte length; production code passes kronika-format's.
    let crc = |b: &[u8]| u32::try_from(b.len()).unwrap_or(u32::MAX);
    let expected = crc(&encoded);
    let decoded = decode_pooled(&pool, id, encoded.len(), expected, crc, |buf| {
        buf.extend_from_slice(&encoded);
    })
    .expect("decode_pooled");
    assert_eq!(decoded.stats.type_id, id);
}

#[test]
fn decode_any_rejects_an_unregistered_type() {
    // Structurally valid (class 2, source 999, version 999) but not in the
    // registry, so decode_any must reject it rather than decode garbage.
    assert!(matches!(
        decode_any(2_999_999, VerifiedSection::for_test(Bytes::new())),
        Err(CodecError::UnknownType { type_id: 2_999_999 })
    ));
}

#[test]
fn section_name_uses_registry_contracts_and_dictionary_names() {
    assert_eq!(section_name(1_021_001), Some("instance_metadata"));
    assert_eq!(section_name(1_104_001), Some("os_meminfo"));
    assert_eq!(section_name(DICT_STRINGS_TYPE_ID), Some("dict.strings"));
    assert_eq!(section_name(DICT_BLOBS_TYPE_ID), Some("dict.blobs"));
    assert_eq!(section_name(9_999_999), None);
}
