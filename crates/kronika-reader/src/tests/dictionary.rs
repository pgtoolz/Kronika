use super::{Dictionary, OwnedDictionaryValue};
use kronika_format::StrId;

#[test]
fn repeated_dictionary_id_requires_the_same_representation() {
    let id = StrId::from_raw(7).expect("nonzero id");
    let mut dictionary = Dictionary::default();
    dictionary
        .insert(id, OwnedDictionaryValue::String(b"same".to_vec()))
        .expect("first value");
    dictionary
        .insert(id, OwnedDictionaryValue::String(b"same".to_vec()))
        .expect("identical repeat");
    assert!(
        dictionary
            .insert(id, OwnedDictionaryValue::String(b"different".to_vec()),)
            .is_err(),
        "a changed value for one id must fail"
    );
}

#[test]
fn repeated_dictionary_id_cannot_change_placement_or_blob_metadata() {
    let id = StrId::from_raw(8).expect("nonzero id");
    let blob = OwnedDictionaryValue::Blob {
        stored_bytes: b"prefix".to_vec(),
        full_len: 12,
        truncated: true,
        full_sha256: Some([3; 32]),
    };
    let mut dictionary = Dictionary::default();
    dictionary.insert(id, blob.clone()).expect("first blob");
    dictionary.insert(id, blob).expect("identical blob");
    assert!(
        dictionary
            .insert(
                id,
                OwnedDictionaryValue::Blob {
                    stored_bytes: b"prefix".to_vec(),
                    full_len: 13,
                    truncated: true,
                    full_sha256: Some([3; 32]),
                },
            )
            .is_err(),
        "changed blob metadata must fail"
    );
    assert!(
        dictionary
            .insert(id, OwnedDictionaryValue::String(b"prefix".to_vec()))
            .is_err(),
        "string/blob placement changes must fail"
    );
}
