//! Section type ids: `type_id = C_SSS_VVV` (README.md, "Type Ids").
//!
//! The decimal digits encode the section class `C`, the source `SSS` within
//! that class, and the layout version `VVV`. For example `1_006_001` is class
//! 1 (snapshot), source 006, version 001.

/// The class of a section: the `C` part of `C_SSS_VVV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionClass {
    /// Snapshot sections (class 1).
    Snapshot,
    /// Event sections (class 2).
    Event,
    /// Dictionary sections (class 3).
    Dictionary,
    /// Chart sections (class 10).
    Chart,
}

impl SectionClass {
    /// The class digit as written in a `type_id`.
    #[must_use]
    pub const fn digit(self) -> u32 {
        match self {
            Self::Snapshot => 1,
            Self::Event => 2,
            Self::Dictionary => 3,
            Self::Chart => 10,
        }
    }

    /// The class for a digit, or `None` if the digit is not a known class.
    #[must_use]
    pub const fn from_digit(digit: u32) -> Option<Self> {
        match digit {
            1 => Some(Self::Snapshot),
            2 => Some(Self::Event),
            3 => Some(Self::Dictionary),
            10 => Some(Self::Chart),
            _ => None,
        }
    }
}

/// A section type id, `C_SSS_VVV` (README.md, "Type Ids").
///
/// Constructed only inside this crate, by `#[derive(Section)]`: the validating
/// constructor rejects an unknown class or a zero source or version, so an
/// invalid id is a compile error and every id in a contract is valid by
/// construction. External code cannot mint one; the accessors below only
/// decompose an existing id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(u32);

impl TypeId {
    /// The sole `TypeId` constructor: wrap a raw id after checking its class,
    /// source, and version. Crate-private so every id traces back to a
    /// registered type; `#[derive(Section)]` turns the `None` case into a
    /// compile error at the contract's `const` site.
    ///
    /// `None` unless the class digit is a known [`SectionClass`], the source
    /// is at least 001, and the version is at least 001 (both start at 001).
    #[must_use]
    pub(crate) const fn new(raw: u32) -> Option<Self> {
        let id = Self(raw);
        if SectionClass::from_digit(id.class_digit()).is_some()
            && id.source() >= 1
            && id.version() >= 1
        {
            Some(id)
        } else {
            None
        }
    }

    /// The raw `type_id` as stored on disk.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The class digit `C`.
    #[must_use]
    pub const fn class_digit(self) -> u32 {
        self.0 / 1_000_000
    }

    /// The section class, or `None` if the class digit is unknown.
    #[must_use]
    pub const fn section_class(self) -> Option<SectionClass> {
        SectionClass::from_digit(self.class_digit())
    }

    /// The source `SSS` within the class.
    #[must_use]
    pub const fn source(self) -> u32 {
        (self.0 / 1_000) % 1_000
    }

    /// The layout version `VVV`.
    #[must_use]
    pub const fn version(self) -> u32 {
        self.0 % 1_000
    }
}

#[cfg(test)]
mod tests {
    use super::{SectionClass, TypeId};

    #[test]
    fn decomposes_the_digits() {
        let id = TypeId::new(1_006_001).expect("valid");
        assert_eq!(id.class_digit(), 1);
        assert_eq!(id.source(), 6);
        assert_eq!(id.version(), 1);
        assert_eq!(id.section_class(), Some(SectionClass::Snapshot));
        assert_eq!(id.get(), 1_006_001);
    }

    #[test]
    fn charts_use_the_two_digit_class() {
        let id = TypeId::new(10_001_001).expect("valid chart id");
        assert_eq!(id.class_digit(), 10);
        assert_eq!(id.section_class(), Some(SectionClass::Chart));
        assert_eq!(id.source(), 1);
        assert_eq!(id.version(), 1);
    }

    #[test]
    fn rejects_unknown_class_zero_source_and_zero_version() {
        // Class 4 is not assigned.
        assert_eq!(TypeId::new(4_000_001), None);
        // Source must start at 1.
        assert_eq!(TypeId::new(1_000_001), None);
        // Version must start at 1.
        assert_eq!(TypeId::new(1_006_000), None);
    }
}
