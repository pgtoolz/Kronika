//! Dictionary decoding in journal order, with one shared body-selection pass.

use std::collections::HashSet;

use kronika_registry::{CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID};

#[cfg(feature = "posix")]
use super::active_body;
use super::{Catalog, Entry, ReaderError, Segment, Source, VerifiedSection, finished_body};
use crate::dictionary::{Dictionary, match_prefix};

impl Segment {
    /// Decode the complete segment dictionary.
    ///
    /// Both `dict.strings` and `dict.blobs` are included. Current-segment
    /// dictionary deltas are applied in journal order.
    ///
    /// # Errors
    /// Returns an error when a dictionary body fails its checksum or codec.
    pub fn dictionary(&self) -> Result<Dictionary, ReaderError> {
        let mut dictionary = Dictionary::default();
        self.visit_dictionary(|entry, body| {
            dictionary.decode(entry.type_id, body, u64::from(entry.rows))
        })?;
        Ok(dictionary)
    }

    /// Decode only dictionary values named by `ids`.
    ///
    /// The dictionary id column is scanned first and large value columns are
    /// projected only for matching rows. An empty set opens no dictionary
    /// section.
    ///
    /// # Errors
    /// Returns an error when a dictionary body fails its checksum or codec.
    pub fn dictionary_for(&self, ids: &HashSet<u64>) -> Result<Dictionary, ReaderError> {
        let mut dictionary = Dictionary::default();
        if !ids.is_empty() {
            self.visit_dictionary(|entry, body| {
                dictionary.decode_selected(entry.type_id, body, ids, u64::from(entry.rows))
            })?;
        }
        Ok(dictionary)
    }

    /// Match the stored bytes of strings and blobs without retaining their values.
    ///
    /// # Errors
    /// Returns an error when a dictionary body fails its checksum or codec.
    pub fn dictionary_ids_with_prefix(&self, prefix: &[u8]) -> Result<HashSet<u64>, ReaderError> {
        let mut matches = HashSet::new();
        self.visit_dictionary(|entry, body| {
            match_prefix(
                entry.type_id,
                body,
                u64::from(entry.rows),
                prefix,
                &mut matches,
            )
        })?;
        Ok(matches)
    }

    /// Decode each dictionary body once while retaining only `ids`.
    ///
    /// This is intended for product queries that have already collected their
    /// compact references during the data-row scan.
    ///
    /// # Errors
    /// Returns an error when a dictionary body fails its checksum or codec.
    pub fn dictionary_once_for(&self, ids: &HashSet<u64>) -> Result<Dictionary, ReaderError> {
        let mut dictionary = Dictionary::default();
        if !ids.is_empty() {
            self.visit_dictionary(|entry, body| {
                dictionary.decode_selected_once(entry.type_id, body, ids, u64::from(entry.rows))
            })?;
        }
        Ok(dictionary)
    }

    fn visit_dictionary(
        &self,
        mut decode: impl FnMut(&Entry, VerifiedSection) -> Result<(), CodecError>,
    ) -> Result<(), ReaderError> {
        match &self.source {
            Source::Finished { bytes, catalog } => {
                decode_catalog(catalog, |entry| finished_body(bytes, entry), &mut decode)
            }
            #[cfg(feature = "posix")]
            Source::Active(snapshot) => {
                for (part_index, part) in snapshot.parts().iter().enumerate() {
                    decode_catalog(
                        &part.catalog,
                        |entry| active_body(snapshot, part_index, entry),
                        &mut decode,
                    )?;
                }
                Ok(())
            }
        }
    }
}

fn decode_catalog(
    catalog: &Catalog,
    mut read_body: impl FnMut(&Entry) -> Result<VerifiedSection, ReaderError>,
    decode: &mut impl FnMut(&Entry, VerifiedSection) -> Result<(), CodecError>,
) -> Result<(), ReaderError> {
    for entry in &catalog.entries {
        if matches!(entry.type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID) {
            decode(entry, read_body(entry)?).map_err(|source| ReaderError::Section {
                type_id: entry.type_id,
                source,
            })?;
        }
    }
    Ok(())
}
